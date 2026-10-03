//! One local typing buffer and at most one accepted worker operation. Input stays
//! responsive while shared Rust fits/renders; only Finish publishes document history.
use super::*;
use captures_app::editor_session::TextInputTarget;

mod outline;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Begin,
    Update,
    Finish,
}

pub(super) struct FlushInput {
    pub input_id: String,
    pub text: String,
    pub commit: bool,
    pub finishing: bool,
}

pub(super) struct InlineText {
    id: String,
    target: TextInputTarget,
    started: bool,
    text: String,
    accepted: String,
    phase: Option<Phase>,
    finish: Option<bool>,
    blocked: bool,
    focus: bool,
    ime_preedit: bool,
    first_frame: Option<u64>,
    close_after: bool,
    previous_selection: Option<String>,
    previous_document: Arc<Document>,
    previous_output: Option<(egui::TextureHandle, u64)>,
    outline: outline::Outline,
}

impl View {
    pub(super) fn begin_inline(&mut self, tx: &Sender<Job>, target: TextInputTarget) {
        if self.pending || self.inline.is_some() || self.closed || self.close_requested {
            return;
        }
        if self
            .text
            .as_ref()
            .is_some_and(|fields| fields.staged.patch(&fields.accepted) != TextPatch::default())
        {
            self.error = Some(
                "Apply or cancel the staged text properties before typing on the canvas.".into(),
            );
            self.section = Section::Layers;
            return;
        }
        let Some(presented) = &self.presented else {
            return;
        };
        let text = match &target {
            TextInputTarget::New { create } => create.text.clone(),
            TextInputTarget::Existing { id } => {
                let Some(Element::Text(element)) = presented
                    .document
                    .elements
                    .iter()
                    .find(|e| &e.base().id == id)
                else {
                    return;
                };
                element.text.clone()
            }
        };
        let id = uuid::Uuid::new_v4().to_string();
        self.inline = Some(InlineText {
            id: id.clone(),
            target: target.clone(),
            started: false,
            text: text.clone(),
            accepted: text,
            phase: Some(Phase::Begin),
            finish: None,
            blocked: false,
            focus: true,
            ime_preedit: false,
            first_frame: None,
            close_after: false,
            previous_selection: self.selected_layer.clone(),
            previous_document: presented.document.clone(),
            previous_output: self.output.clone(),
            outline: Default::default(),
        });
        self.cancel_edit_gestures();
        self.cancel_crop();
        self.viewport_pan = None;
        self.submit(
            tx,
            Request::BeginTextInput {
                input_id: id,
                target,
            },
        );
        if !self.pending {
            self.inline_failed();
        }
    }

    pub(super) fn finish_inline(&mut self, commit: bool) {
        if let Some(input) = &mut self.inline {
            if input.phase == Some(Phase::Finish) {
                return;
            }
            input.finish = Some(commit);
            input.blocked = false;
        }
    }

    pub(super) fn drain_inline(&mut self, tx: &Sender<Job>) {
        if self.pending || self.closed {
            return;
        }
        let Some(input) = &mut self.inline else {
            return;
        };
        if input.blocked || input.phase.is_some() {
            return;
        }
        // Shipping has no Cancel: after a failed Begin, finishing a blank box
        // dismisses it (nothing reached the document); other text retries.
        if !input.started
            && (input.finish == Some(false)
                || (input.finish == Some(true) && input.text.trim().is_empty()))
        {
            let input = self.inline.take().unwrap();
            self.output = input.previous_output;
            self.select_layer_exact(input.previous_selection);
            self.error = None;
            return;
        }
        let request = if !input.started {
            input.phase = Some(Phase::Begin);
            Request::BeginTextInput {
                input_id: input.id.clone(),
                target: input.target.clone(),
            }
        } else if input.finish == Some(false) {
            input.phase = Some(Phase::Finish);
            Request::FinishTextInput {
                input_id: input.id.clone(),
                commit: false,
            }
        } else if input.text != input.accepted {
            input.phase = Some(Phase::Update);
            Request::UpdateTextInput {
                input_id: input.id.clone(),
                text: input.text.clone(),
            }
        } else if input.finish == Some(true) {
            input.phase = Some(Phase::Finish);
            Request::FinishTextInput {
                input_id: input.id.clone(),
                commit: true,
            }
        } else {
            return;
        };
        self.submit(tx, request);
        if !self.pending {
            self.inline_failed();
        }
    }

    pub(super) fn received_inline(&mut self) {
        let Some(mut input) = self.inline.take() else {
            return;
        };
        let Some(presented) = &self.presented else {
            self.inline = Some(input);
            return;
        };
        if let Some((token, id)) = &presented.active_text_input {
            if token != &input.id {
                self.inline = Some(input);
                return;
            }
            if let Some(Element::Text(element)) = presented
                .document
                .elements
                .iter()
                .find(|e| &e.base().id == id)
            {
                input.accepted = element.text.clone();
            }
            input.phase = None;
            input.started = true;
            let id = id.clone();
            self.inline = Some(input);
            self.select_layer(Some(id));
        } else {
            // Cancellation and unchanged/blank-new composition retain encoded
            // output; a real document change goes through normal invalidation.
            if presented.document == input.previous_document {
                self.output = input.previous_output;
                self.select_layer_exact(input.previous_selection);
            }
            if input.close_after {
                self.request_close();
            }
        }
    }

    pub(super) fn inline_failed(&mut self) {
        let Some(input) = &mut self.inline else {
            return;
        };
        // Only a finish attempt releases focus. Ordinary preview failures must
        // not re-request it over a still-active selection or clipboard command.
        input.focus |= input.finish.is_some();
        input.phase = None;
        input.finish = None;
        input.blocked = true;
        input.close_after = false;
    }

    pub(super) fn close_inline(&mut self) -> bool {
        let Some(input) = &mut self.inline else {
            return false;
        };
        input.close_after = true;
        self.finish_inline(true);
        true
    }

    pub(super) fn inline_for_flush(&self) -> Option<FlushInput> {
        self.inline.as_ref().map(|input| FlushInput {
            input_id: input.id.clone(),
            text: input.text.clone(),
            commit: input.finish != Some(false),
            finishing: input.phase == Some(Phase::Finish),
        })
    }
}

impl View {
    /// The text layer the inline editor draws: the session's transient layer
    /// once Begin is accepted, else the existing layer or the unfitted layer a
    /// Text click creates (`new_text_element`), so the box appears at once.
    fn inline_element(&self) -> Option<TextElement> {
        let input = self.inline.as_ref()?;
        let presented = self.presented.as_ref()?;
        let find = |id: &str| {
            presented
                .document
                .elements
                .iter()
                .find_map(|element| match element {
                    Element::Text(text) if text.base.id == id => Some(text.clone()),
                    _ => None,
                })
        };
        match (&presented.active_text_input, &input.target) {
            (Some((token, id)), _) if token == &input.id => find(id),
            (_, TextInputTarget::Existing { id }) => find(id),
            (_, TextInputTarget::New { create }) => {
                captures_app::editor_session::new_text_element(String::new(), create).ok()
            }
        }
    }
}

/// Where and how the inline editor draws, in screen points.
struct InlineGeometry {
    /// The unrotated frame (plate or glyph box, at least 48 × 28 pt).
    frame: egui::Rect,
    /// The text box inside the frame's padding.
    content: egui::Rect,
    /// Clockwise radians about the frame centre.
    angle: f32,
    format: egui::TextFormat,
    halign: egui::Align,
    auto_width: bool,
    outline_width: Option<f32>,
    plate: Option<(egui::Color32, f32)>,
}

fn inline_geometry(
    ctx: &egui::Context,
    element: &TextElement,
    font_name: Option<&str>,
    text: &str,
    preview: egui::Rect,
    scale: f32,
) -> Option<InlineGeometry> {
    use captures_app::editor_text::{INLINE_EDITOR_MIN_SIZE, inline_editor_layout};
    let layout = inline_editor_layout(element).ok()?;
    let [top, right, bottom, left] = layout.padding.map(|value| value as f32 * scale);
    let alpha = (element.base.opacity / 100.).clamp(0., 1.) as f32;
    let color = |value: &str| {
        egui::Color32::from_hex(value)
            .unwrap_or(egui::Color32::BLACK)
            .gamma_multiply(alpha)
    };
    let format = egui::TextFormat {
        font_id: egui::FontId::new(
            element.font_size as f32 * scale,
            crate::ui_fonts::editor_text_family(
                ctx,
                &element.font_family,
                font_name,
                element.bold,
                element.italic,
            ),
        ),
        color: color(&element.color),
        line_height: Some(layout.line_height as f32 * scale),
        ..Default::default()
    };
    let halign = match element.align.as_str() {
        "center" => egui::Align::Center,
        "right" => egui::Align::RIGHT,
        _ => egui::Align::LEFT,
    };
    let frame = layout.frame;
    let accepted_width = frame.width as f32 * scale - left - right;
    let galley = inline_galley(
        ctx,
        text,
        &format,
        halign,
        layout.auto_width,
        accepted_width,
    );
    // Auto-width labels grow with the typed buffer before the session refits
    // them; the accepted box keeps its left edge, centre or right edge.
    let content_width = if layout.auto_width {
        galley.size().x.ceil() + 2.
    } else {
        accepted_width
    };
    let line_height = layout.line_height as f32 * scale;
    let content_height = (galley.rows.len().max(1) as f32 * line_height).max(line_height);
    let width = (content_width + left + right).max(INLINE_EDITOR_MIN_SIZE.0 as f32);
    let height = (content_height + top + bottom).max(INLINE_EDITOR_MIN_SIZE.1 as f32);
    let accepted_left = frame.x as f32 * scale;
    let accepted_screen_width = frame.width as f32 * scale;
    let x = if layout.auto_width {
        match halign {
            egui::Align::Center => accepted_left + (accepted_screen_width - width) / 2.,
            egui::Align::RIGHT => accepted_left + accepted_screen_width - width,
            egui::Align::LEFT => accepted_left,
        }
    } else {
        accepted_left
    };
    let frame = egui::Rect::from_min_size(
        preview.min + egui::vec2(x, frame.y as f32 * scale),
        egui::vec2(width, height),
    );
    let content = egui::Rect::from_min_max(
        frame.min + egui::vec2(left, top),
        frame.max - egui::vec2(right, bottom),
    );
    Some(InlineGeometry {
        frame,
        content,
        angle: layout.rotation as f32,
        format,
        halign,
        auto_width: layout.auto_width,
        outline_width: element
            .outlined
            .then_some(layout.outline_width as f32 * scale),
        plate: element
            .background
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(|value| (color(value), layout.plate_radius as f32 * scale)),
    })
}

fn inline_galley(
    ctx: &egui::Context,
    text: &str,
    format: &egui::TextFormat,
    halign: egui::Align,
    auto_width: bool,
    wrap_width: f32,
) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(text.to_owned(), format.clone());
    job.wrap.max_width = if auto_width {
        f32::INFINITY
    } else {
        wrap_width
    };
    job.halign = halign;
    job.keep_trailing_whitespace = true;
    ctx.fonts_mut(|fonts| fonts.layout_job(job))
}

/// Shipping `.screenshot-inline-text-frame`: the text being typed sits on the
/// canvas in the layer's own face, size, colour, plate and rotation, inside a
/// 1 px accent outline `--s-3` outside the frame. Clicking away and Escape
/// commit; Enter inserts a new line. The session preview omits this layer
/// while the input is active.
///
/// Rotated labels paint their glyphs, plate and caret rotated about the frame
/// centre; the selection highlight and pointer caret placement use the
/// unrotated box (egui text fields cannot rotate). Outlined labels replace
/// only the visible glyph ink with cached, hollow atlas-derived strokes.
pub(super) fn show(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    available: egui::Rect,
    preview: egui::Rect,
) {
    let Some(element) = view.inline_element() else {
        return;
    };
    let scale = preview.width() / view.canvas[0].max(1.) as f32;
    let font_name = view
        .presented
        .as_ref()
        .and_then(|presented| presented.font_families.get(&element.font_family).cloned());
    let Some(input) = &mut view.inline else {
        return;
    };
    let Some(geometry) = inline_geometry(
        ui.ctx(),
        &element,
        font_name.as_deref(),
        &input.text,
        preview,
        scale,
    ) else {
        return;
    };
    let input_id = ui.scope_id().with((&input.id, "canvas-text-input"));
    let finishing = input.phase == Some(Phase::Finish);
    let frame_nr = ui.ctx().cumulative_frame_nr();
    let first_frame = *input.first_frame.get_or_insert(frame_nr) == frame_nr;
    let blocked = input.blocked;
    // A backend may deliver Escape alongside a preedit dismissal/commit. That
    // key belongs to the IME, not the document's Finish action.
    let mut ime_owned_escape = input.ime_preedit;
    if ui.ctx().current_pass_index() == 0 {
        ui.input(|i| {
            for event in &i.events {
                match event {
                    egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => {
                        input.ime_preedit = !text.is_empty();
                        ime_owned_escape |= input.ime_preedit;
                    }
                    egui::Event::Ime(egui::ImeEvent::Commit(_)) => {
                        input.ime_preedit = false;
                        ime_owned_escape = true;
                    }
                    _ => {}
                }
            }
        });
    }
    let rotated = geometry.angle.abs() > f32::EPSILON;
    let pivot = geometry.frame.center();
    let rotation = egui::emath::Rot2::from_angle(geometry.angle);
    let rotate = |point: egui::Pos2| pivot + rotation * (point - pivot);
    let text_color = geometry.format.color;
    let separate_ink = rotated || geometry.outline_width.is_some();
    let mut lost_focus = false;
    let response = egui::Area::new(ui.scope_id().with((&input.id, "canvas-text-frame")))
        .order(egui::Order::Foreground)
        .fixed_pos(geometry.frame.min)
        .constrain(false)
        .show(ui.ctx(), |ui| {
            ui.set_clip_rect(available);
            let painter = ui.painter().clone();
            painter.add(
                egui::epaint::RectShape::stroke(
                    geometry.frame.expand(tokens.number("s-3")),
                    tokens.number("r-sm"),
                    egui::Stroke::new(1., tokens.color("theme-accent")),
                    egui::StrokeKind::Inside,
                )
                .with_angle(geometry.angle),
            );
            if let Some((fill, radius)) = geometry.plate {
                painter.add(
                    egui::epaint::RectShape::filled(geometry.frame, radius, fill)
                        .with_angle(geometry.angle),
                );
            }
            let visuals = ui.visuals_mut();
            visuals.selection.bg_fill = if rotated {
                egui::Color32::TRANSPARENT
            } else {
                tokens.color("theme-accent").gamma_multiply(0.2)
            };
            // Selection recolors glyph vertices independently of TextFormat.
            // Keep its background without restoring a filled outlined label.
            visuals.selection.stroke.color = if separate_ink {
                egui::Color32::TRANSPARENT
            } else {
                text_color
            };
            visuals.text_cursor.stroke = egui::Stroke::new(
                (geometry.format.font_id.size / 16.).clamp(1., 3.),
                if rotated {
                    egui::Color32::TRANSPARENT
                } else {
                    text_color
                },
            );
            let mut format = geometry.format.clone();
            if separate_ink {
                format.color = egui::Color32::TRANSPARENT;
            }
            let (halign, auto_width, wrap) = (
                geometry.halign,
                geometry.auto_width,
                geometry.content.width(),
            );
            let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, _: f32| {
                inline_galley(ui.ctx(), buffer.as_str(), &format, halign, auto_width, wrap)
            };
            let output = ui
                .scope_builder(egui::UiBuilder::new().max_rect(geometry.content), |ui| {
                    egui::TextEdit::multiline(&mut input.text)
                        .id(input_id)
                        .frame(egui::Frame::NONE)
                        .interactive(!finishing)
                        .align(egui::Align2([halign, egui::Align::TOP]))
                        .layouter(&mut layouter)
                        .desired_width(geometry.content.width())
                        .min_size(geometry.content.size())
                        // Keep focus until earlier queued edits have run.
                        // The host handles Escape after the field below.
                        .event_filter(egui::EventFilter {
                            horizontal_arrows: true,
                            vertical_arrows: true,
                            escape: true,
                            ..Default::default()
                        })
                        .show(ui)
                })
                .inner;
            let field = &output.response.response;
            field.widget_info(|| {
                egui::WidgetInfo::text_edit(
                    true,
                    "",
                    input.text.as_str(),
                    captures_app::editor_chrome::text_format::INLINE_LABEL,
                )
            });
            if input.focus {
                field.request_focus();
                input.focus = false;
            }
            if field.changed() {
                input.blocked = false;
                // Geometry above used the buffer before TextEdit handled this
                // pass's events. Refit before presenting the new glyphs, not
                // after a later pointer event or cursor blink.
                ui.ctx().request_discard("inline typing refits its frame");
            }
            lost_focus = field.lost_focus();
            if field.has_focus() {
                // The accent outline is the indicator (`outline: 0` on the textarea).
                crate::primitives::focus_indicated(ui.ctx());
            }
            if separate_ink {
                let origin = output.galley_pos - egui::vec2(output.galley.rect.left(), 0.);
                if let Some(width) = geometry.outline_width {
                    input.outline.paint(
                        &painter,
                        &output.galley,
                        origin,
                        width,
                        text_color,
                        rotate,
                    );
                } else {
                    let mut shape = egui::epaint::TextShape::new(
                        rotate(origin),
                        output.galley.clone(),
                        text_color,
                    );
                    shape.override_text_color = Some(text_color);
                    shape.angle = geometry.angle;
                    painter.add(shape);
                }
                if rotated
                    && field.has_focus()
                    && let Some(range) = output.cursor_range
                {
                    let caret = output
                        .galley
                        .pos_from_cursor(range.primary)
                        .translate(origin.to_vec2());
                    painter.line_segment(
                        [rotate(caret.center_top()), rotate(caret.center_bottom())],
                        egui::Stroke::new(
                            (geometry.format.font_id.size / 16.).clamp(1., 3.),
                            text_color,
                        ),
                    );
                }
            }
        })
        .response;
    let mut finish = None;
    if ui.ctx().current_pass_index() == 0 && !finishing && !first_frame {
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            if !ime_owned_escape {
                finish = Some(true);
            }
        } else if !blocked
            && (!ui.input(|i| i.focused) || response.clicked_elsewhere() || lost_focus)
        {
            finish.get_or_insert(true);
        }
    }
    if let Some(commit) = finish {
        // Shipping blurs the textarea before finishing. Unregistering it on a
        // later idle frame is too late for the first document shortcut; only
        // surrender this field's focus, preserving a newly clicked control.
        ui.memory_mut(|memory| memory.surrender_focus(input_id));
        view.finish_inline(commit);
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::presented_text;
    use super::*;

    fn setup() -> (egui::Context, View, Sender<Job>, Receiver<Job>) {
        let ctx = egui::Context::default();
        let mut view = View::default();
        view.receive(&ctx, Ok(presented_text("label", "original")));
        view.output = Some((view.texture.as_ref().unwrap().clone(), 37));
        let (tx, rx) = mpsc::channel();
        view.begin_inline(&tx, TextInputTarget::Existing { id: "label".into() });
        assert!(matches!(
            rx.try_recv(),
            Ok(Job::Apply(Request::BeginTextInput { .. }))
        ));
        (ctx, view, tx, rx)
    }

    fn accept(ctx: &egui::Context, view: &mut View, text: &str) {
        let mut presented = presented_text("label", text);
        presented.active_text_input =
            Some((view.inline.as_ref().unwrap().id.clone(), "label".into()));
        view.receive(ctx, Ok(presented));
    }

    #[test]
    fn inline_geometry_follows_the_layer_style_anchor_rotation_and_minimum() {
        let ctx = egui::Context::default();
        crate::ui_fonts::install(&ctx);
        ctx.begin_pass(Default::default());
        let Some(Element::Text(mut element)) = presented_text("label", "Wide label text")
            .document
            .elements
            .last()
            .cloned()
        else {
            unreachable!()
        };
        element.font_family = "sans".into();
        element.align = "center".into();
        element.background = Some("#111318".into());
        element.rounded_background = true;
        element.base.rotation = Some(0.5);
        element.base.opacity = 50.;
        let preview = egui::Rect::from_min_size(egui::pos2(100., 50.), egui::vec2(320., 180.));
        let geometry =
            inline_geometry(&ctx, &element, None, "Wide label text", preview, 0.5).unwrap();
        let layout = captures_app::editor_text::inline_editor_layout(&element).unwrap();
        // Auto width keeps the accepted plate's centre while the buffer grows.
        let accepted_center =
            preview.left() + (layout.frame.x + layout.frame.width / 2.) as f32 * 0.5;
        assert!((geometry.frame.center().x - accepted_center).abs() < 0.5);
        assert!(
            (geometry.frame.top() - (preview.top() + layout.frame.y as f32 * 0.5)).abs() < 1e-3
        );
        assert_eq!(geometry.angle, 0.5);
        assert_eq!(geometry.halign, egui::Align::Center);
        assert_eq!(geometry.format.font_id.size, 16.);
        assert_ne!(
            geometry.format.font_id.family,
            egui::FontFamily::Proportional
        );
        let (plate, radius) = geometry.plate.unwrap();
        assert!(plate.a() < 255 && radius > 0.);
        assert!(geometry.content.width() < geometry.frame.width());
        assert!(geometry.outline_width.is_none());
        element.outlined = true;
        let outlined =
            inline_geometry(&ctx, &element, None, "Wide label text", preview, 0.5).unwrap();
        assert_eq!(outlined.outline_width, Some(1.28));
        assert_eq!(
            outlined.frame, geometry.frame,
            "stroke cannot move the anchor"
        );
        assert_eq!(
            outlined.content, geometry.content,
            "stroke cannot change wrapping"
        );
        assert_eq!(outlined.angle, geometry.angle);
        assert_eq!(outlined.format, geometry.format);
        // Shipping's 48 × 28 minimum applies to a blank label.
        let blank = inline_geometry(&ctx, &element, None, "", preview, 0.1).unwrap();
        assert!(blank.frame.width() > 47.99 && blank.frame.height() > 27.99);
        ctx.end_pass().textures_delta.clear();
    }

    #[test]
    fn coalesces_typing_and_finishes_the_newest_buffer_not_the_inflight_preview() {
        let (ctx, mut view, tx, rx) = setup();
        view.inline.as_mut().unwrap().text = "first".into();
        view.drain_inline(&tx);
        assert!(rx.try_recv().is_err(), "Begin is still in flight");
        accept(&ctx, &mut view, "original");
        view.drain_inline(&tx);
        assert!(
            matches!(rx.try_recv(), Ok(Job::Apply(Request::UpdateTextInput { text, .. })) if text == "first")
        );
        view.inline.as_mut().unwrap().text = "newest\nαβ".into();
        view.finish_inline(true);
        view.drain_inline(&tx);
        assert!(rx.try_recv().is_err());
        accept(&ctx, &mut view, "first");
        assert_eq!(view.inline.as_ref().unwrap().text, "newest\nαβ");
        view.drain_inline(&tx);
        assert!(
            matches!(rx.try_recv(), Ok(Job::Apply(Request::UpdateTextInput { text, .. })) if text == "newest\nαβ")
        );
        accept(&ctx, &mut view, "newest\nαβ");
        view.drain_inline(&tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Job::Apply(Request::FinishTextInput { commit: true, .. }))
        ));
        view.drain_inline(&tx);
        assert!(rx.try_recv().is_err());
        view.receive(&ctx, Ok(presented_text("label", "newest\nαβ")));
        assert!(view.inline.is_none() && view.output.is_none());
        assert_eq!(view.selected_layer.as_deref(), Some("label"));
        assert_eq!(
            view.section,
            Section::Layers,
            "editing from Select keeps Select"
        );
    }

    #[test]
    fn new_text_keeps_its_tool_after_finish_live_properties_and_autosave() {
        let ctx = egui::Context::default();
        let mut view = View {
            autosaves: true,
            ..View::default()
        };
        view.receive(&ctx, Ok(super::super::tests::presented(false)));
        view.activate_tool(Section::Draw, Some(DrawShape::Text));
        let (tx, rx) = mpsc::channel();
        view.begin_inline(
            &tx,
            TextInputTarget::New {
                create: TextCreate {
                    point: Point { x: 2., y: 1. },
                    text: String::new(),
                    font_size: 32.,
                    font_family: "sans".into(),
                    color: "#ff3b5c".into(),
                    style_preset: None,
                    drop_shadow: None,
                    drop_shadow_style: None,
                },
            },
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(Job::Apply(Request::BeginTextInput { .. }))
        ));
        view.inline.as_mut().unwrap().text = "finished".into();
        accept(&ctx, &mut view, "");
        view.drain_inline(&tx);
        assert!(
            matches!(rx.try_recv(), Ok(Job::Apply(Request::UpdateTextInput { text, .. })) if text == "finished")
        );
        accept(&ctx, &mut view, "finished");
        view.finish_inline(true);
        view.drain_inline(&tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Job::Apply(Request::FinishTextInput { commit: true, .. }))
        ));
        view.receive(&ctx, Ok(presented_text("label", "finished")));
        assert!(view.inline.is_none());
        assert_eq!(
            (view.section, view.draw_shape),
            (Section::Draw, DrawShape::Text)
        );
        assert!(!view.tool_shows_transform_chrome());
        let mut edited = presented_text("label", "Properties edit");
        edited.created_layer = None;
        view.receive(&ctx, Ok(edited));
        assert_eq!(
            view.section,
            Section::Draw,
            "a live property reply keeps Text"
        );
        view.autosave.edited(Instant::now() - DraftAutosave::DELAY);
        view.drive_autosave(&ctx, &tx);
        let Ok(Job::Autosave { reply }) = rx.try_recv() else {
            panic!("a due text autosave reaches the worker");
        };
        reply.send(Ok(true)).unwrap();
        assert!(view.receive_autosave());
        assert_eq!(view.section, Section::Draw);
        assert_eq!(view.selected_layer.as_deref(), Some("label"));
        assert_eq!(view.text.as_ref().unwrap().accepted.text, "Properties edit");
    }

    #[test]
    fn cancel_does_not_submit_unaccepted_text_and_restores_output() {
        let (ctx, mut view, tx, rx) = setup();
        accept(&ctx, &mut view, "original");
        view.inline.as_mut().unwrap().text = "never published".into();
        view.finish_inline(false);
        view.drain_inline(&tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Job::Apply(Request::FinishTextInput { commit: false, .. }))
        ));
        assert!(rx.try_recv().is_err());
        view.receive(&ctx, Ok(presented_text("label", "original")));
        assert!(view.inline.is_none() && view.output.is_some());
        assert_eq!(view.output.as_ref().unwrap().1, 37);
        assert_eq!(view.selected_layer.as_deref(), Some("label"));
    }

    #[test]
    fn failures_retain_typing_without_automatic_retries_and_cancel_failed_begin_locally() {
        let (ctx, mut view, tx, rx) = setup();
        view.inline.as_mut().unwrap().text = "typed during begin".into();
        view.receive(&ctx, Err("font unavailable".into()));
        view.drain_inline(&tx);
        assert!(rx.try_recv().is_err());
        assert_eq!(view.inline.as_ref().unwrap().text, "typed during begin");
        view.finish_inline(true);
        view.drain_inline(&tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Job::Apply(Request::BeginTextInput { .. }))
        ));
        view.receive(&ctx, Err("still unavailable".into()));
        view.finish_inline(false);
        view.drain_inline(&tx);
        assert!(view.inline.is_none() && view.output.is_some() && rx.try_recv().is_err());

        // Without a Cancel button, clearing the box and finishing dismisses it.
        let (ctx, mut view, tx, rx) = setup();
        view.receive(&ctx, Err("font unavailable".into()));
        view.inline.as_mut().unwrap().text = " \n".into();
        view.finish_inline(true);
        view.drain_inline(&tx);
        assert!(view.inline.is_none() && view.output.is_some() && rx.try_recv().is_err());

        let (ctx, mut view, tx, rx) = setup();
        accept(&ctx, &mut view, "original");
        view.inline.as_mut().unwrap().text = "retryable".into();
        view.request_close();
        view.drain_inline(&tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Job::Apply(Request::UpdateTextInput { .. }))
        ));
        view.receive(&ctx, Err("render failed".into()));
        assert!(!view.closed && !view.close_requested);
        assert_eq!(view.inline.as_ref().unwrap().text, "retryable");
        view.drain_inline(&tx);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn composition_blocks_actions_and_close_defers_until_latest_text_finishes() {
        let (ctx, mut view, tx, rx) = setup();
        accept(&ctx, &mut view, "original");
        view.inline.as_mut().unwrap().text = "latest for quit".into();
        view.copy(&tx);
        view.save(&tx);
        view.submit(&tx, Request::Undo);
        view.submit(&tx, Request::SaveDraft { updated_at_ms: 7 });
        assert!(rx.try_recv().is_err());
        let input = view.inline_for_flush().unwrap();
        assert_eq!(input.input_id, view.inline.as_ref().unwrap().id);
        assert_eq!(input.text, "latest for quit");
        assert!(input.commit && !input.finishing);
        view.request_close();
        assert!(!view.closed && !view.close_requested);
        view.drain_inline(&tx);
        assert!(
            matches!(rx.try_recv(), Ok(Job::Apply(Request::UpdateTextInput { text, .. })) if text == "latest for quit")
        );
        accept(&ctx, &mut view, "latest for quit");
        view.drain_inline(&tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Job::Apply(Request::FinishTextInput { commit: true, .. }))
        ));
        view.receive(&ctx, Ok(presented_text("label", "latest for quit")));
        assert!(view.inline.is_none() && view.close_requested && !view.closed);
    }

    #[test]
    fn quit_saves_the_latest_buffer_even_before_begin_or_update_is_presented() {
        for (update_in_flight, cancel) in [(false, false), (true, false), (true, true)] {
            let data = tempfile::tempdir().unwrap();
            let root = data.path().join("history");
            let artifact = captures_app::persist_screenshot(
                &root,
                &RgbaImage::new(320, 180),
                CaptureMode::Region,
            )
            .unwrap();
            let ctx = egui::Context::default();
            let editor = Editor::open(
                &ctx,
                root,
                artifact.entry.id.clone(),
                data.path().join("exports"),
                CaptureMode::Region,
                |_| unreachable!("quit must not copy pixels"),
            );
            let receive = || {
                let reply = editor
                    .rx
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .unwrap();
                editor.view.lock().unwrap().receive(&ctx, reply);
            };
            receive();
            editor.view.lock().unwrap().begin_inline(
                &editor.tx,
                TextInputTarget::New {
                    create: TextCreate {
                        point: Point { x: 23., y: 31. },
                        text: String::new(),
                        font_size: 24.,
                        font_family: "sans".into(),
                        color: "#2367ab".into(),
                        style_preset: None,
                        drop_shadow: None,
                        drop_shadow_style: None,
                    },
                },
            );
            if update_in_flight {
                receive();
                let mut view = editor.view.lock().unwrap();
                view.inline.as_mut().unwrap().text = "older queued preview".into();
                view.drain_inline(&editor.tx);
            }
            editor.view.lock().unwrap().inline.as_mut().unwrap().text = "Latest\nαβ".into();
            if cancel {
                editor.view.lock().unwrap().finish_inline(false);
            }
            editor.flush(&ctx).unwrap();
            if cancel {
                assert!(
                    !data.path().join("editor-drafts").exists(),
                    "quit must honor pending Cancel"
                );
                continue;
            }
            let manifest: serde_json::Value = serde_json::from_slice(
                &std::fs::read(
                    data.path()
                        .join("editor-drafts")
                        .join(artifact.entry.id)
                        .join("manifest.json"),
                )
                .unwrap(),
            )
            .unwrap();
            let elements = manifest["document"]["elements"].as_array().unwrap();
            assert_eq!(elements.len(), 2);
            assert_eq!(elements[1]["text"], "Latest\nαβ");
            assert_eq!(elements[1]["x"], 23.);
            assert_eq!(elements[1]["y"], 31.);
            assert!(!data.path().join("exports").exists());
        }
    }

    #[test]
    fn the_first_document_undo_after_inline_finish_is_not_owned_by_the_removed_field() {
        for escape in [false, true] {
            let (ctx, mut view, tx, rx) = setup();
            let tokens = crate::tokens::load()["light-mustard"].clone();
            tokens.apply(&ctx, true);
            accept(&ctx, &mut view, "original");
            let other_id = std::cell::Cell::new(egui::Id::NULL);
            let frame = |view: &mut View, events| {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(760., 540.),
                        )),
                        focused: true,
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        super::super::handle_document_shortcuts(&ctx, view, &tx);
                        egui::CentralPanel::default().show(ui, |ui| {
                            let other = ui.put(
                                egui::Rect::from_min_size(
                                    egui::pos2(680., 380.),
                                    egui::vec2(60., 40.),
                                ),
                                egui::Button::new("Other"),
                            );
                            other_id.set(other.id);
                            if other.clicked() {
                                other.request_focus();
                            }
                            let area = ui.available_rect_before_wrap();
                            let preview = egui::Rect::from_min_size(area.min, egui::vec2(7., 3.));
                            show(ui, &tokens, view, area, preview);
                        });
                        if ctx.current_pass_index() == 0 {
                            ctx.request_discard("inline focus multi-pass");
                        }
                    },
                );
                output.textures_delta.clear();
                output.platform_output.commands
            };
            let key = |key, modifiers| egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            };
            frame(&mut view, vec![]);
            assert!(ctx.text_edit_focused());
            frame(
                &mut view,
                vec![
                    key(egui::Key::A, egui::Modifiers::COMMAND),
                    egui::Event::Text("Unaccepted\nbuffer".into()),
                ],
            );
            view.drain_inline(&tx);
            assert!(matches!(
                rx.try_recv(),
                Ok(Job::Apply(Request::UpdateTextInput { .. }))
            ));
            view.receive(&ctx, Err("preview failed".into()));
            assert!(
                !view.inline.as_ref().unwrap().focus,
                "preview errors keep existing focus"
            );
            let copied = frame(
                &mut view,
                vec![
                    key(egui::Key::A, egui::Modifiers::COMMAND),
                    egui::Event::Copy,
                ],
            );
            assert!(
                copied.iter().any(|command| matches!(
                    command,
                    egui::OutputCommand::CopyText(text) if text == "Unaccepted\nbuffer"
                )),
                "Select All/Copy retains the unaccepted buffer after a preview error"
            );
            assert!(
                rx.try_recv().is_err(),
                "clipboard keys belong to inline text"
            );
            // A changed buffer clears the preview-error guard for click-away.
            frame(
                &mut view,
                vec![
                    key(egui::Key::A, egui::Modifiers::COMMAND),
                    egui::Event::Text("Revised\nline two".into()),
                ],
            );
            let outside = egui::pos2(700., 400.);
            let finish = if escape {
                vec![key(egui::Key::Escape, egui::Modifiers::NONE)]
            } else {
                vec![
                    egui::Event::PointerMoved(outside),
                    egui::Event::PointerButton {
                        pos: outside,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                    egui::Event::PointerButton {
                        pos: outside,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]
            };
            frame(&mut view, finish);
            assert_eq!(view.inline.as_ref().unwrap().finish, Some(true));
            view.drain_inline(&tx);
            assert!(
                matches!(
                    rx.try_recv(),
                    Ok(Job::Apply(Request::UpdateTextInput { .. }))
                ),
                "finishing retries the failed preview first"
            );
            accept(&ctx, &mut view, "Revised\nline two");
            view.drain_inline(&tx);
            assert!(matches!(
                rx.try_recv(),
                Ok(Job::Apply(Request::FinishTextInput { commit: true, .. }))
            ));
            assert!(!ctx.text_edit_focused());
            if !escape {
                assert_eq!(
                    ctx.memory(|memory| memory.focused()),
                    Some(other_id.get()),
                    "click-away preserves the new control's focus"
                );
            }
            view.receive(&ctx, Err("finish failed".into()));
            frame(&mut view, vec![]);
            assert!(ctx.text_edit_focused(), "a failed finish remains editable");
            assert_eq!(view.inline.as_ref().unwrap().text, "Revised\nline two");
            view.drain_inline(&tx);
            assert!(
                rx.try_recv().is_err(),
                "failure must not automatically retry"
            );
            frame(
                &mut view,
                vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
            );
            view.drain_inline(&tx);
            assert!(matches!(
                rx.try_recv(),
                Ok(Job::Apply(Request::FinishTextInput { commit: true, .. }))
            ));
            // The worker can finish before an idle frame unregisters the field.
            view.receive(&ctx, Ok(presented_text("label", "Revised\nline two")));
            assert!(view.inline.is_none());
            frame(&mut view, vec![key(egui::Key::Z, egui::Modifiers::COMMAND)]);
            assert!(
                matches!(rx.try_recv(), Ok(Job::Apply(Request::Undo))),
                "first Undo after {} must reach document history",
                if escape { "Escape" } else { "click-away" }
            );
            assert!(rx.try_recv().is_err(), "layout passes must not repeat Undo");
        }
    }

    #[test]
    fn native_input_accepts_multiline_text_while_begin_is_pending_across_layout_passes() {
        let (ctx, mut view, tx, rx) = setup();
        let tokens = crate::tokens::load()["light-mustard"].clone();
        tokens.apply(&ctx, true);
        let frame = |view: &mut View, events| {
            let mut result = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(760., 540.),
                    )),
                    focused: true,
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        let area = ui.available_rect_before_wrap();
                        show(ui, &tokens, view, area, area);
                    });
                    if ctx.current_pass_index() == 0 {
                        ctx.request_discard("input multi-pass");
                    }
                },
            );
            result.textures_delta.clear();
        };
        frame(&mut view, vec![]);
        frame(&mut view, vec![egui::Event::Text("new\nαβ".into())]);
        assert_eq!(view.inline.as_ref().unwrap().text, "originalnew\nαβ");
        assert!(view.inline.as_ref().unwrap().finish.is_none());
        let key = |key, modifiers| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        frame(
            &mut view,
            vec![
                egui::Event::Ime(egui::ImeEvent::Preedit {
                    text: "candidate".into(),
                    active_range_chars: None,
                }),
                key(egui::Key::Escape, egui::Modifiers::NONE),
            ],
        );
        assert!(
            view.inline.as_ref().unwrap().finish.is_none(),
            "IME owns Escape"
        );
        frame(
            &mut view,
            vec![
                egui::Event::Ime(egui::ImeEvent::Commit("é".into())),
                key(egui::Key::Escape, egui::Modifiers::NONE),
            ],
        );
        assert!(view.inline.as_ref().unwrap().finish.is_none());
        assert_eq!(view.inline.as_ref().unwrap().text, "originalnew\nαβé");
        // A fast select-all/delete/Escape sequence can arrive in one repaint.
        // Finishing must include the deletion rather than the preceding preview.
        frame(
            &mut view,
            vec![
                key(egui::Key::A, egui::Modifiers::COMMAND),
                key(egui::Key::Backspace, egui::Modifiers::NONE),
                key(egui::Key::Escape, egui::Modifiers::NONE),
            ],
        );
        assert_eq!(view.inline.as_ref().unwrap().text, "");
        assert_eq!(view.inline.as_ref().unwrap().finish, Some(true));
        view.drain_inline(&tx);
        assert!(
            rx.try_recv().is_err(),
            "typing must not enqueue ahead of Begin"
        );
    }

    #[test]
    fn typing_refits_the_frame_in_the_same_render_not_on_a_later_pointer_event() {
        let (ctx, mut view, _, _) = setup();
        crate::ui_fonts::install(&ctx);
        let tokens = crate::tokens::load()["light-mustard"].clone();
        tokens.apply(&ctx, true);
        view.canvas = [640., 360.];
        let document = Arc::make_mut(&mut view.presented.as_mut().unwrap().document);
        let Element::Text(element) = document.elements.last_mut().unwrap() else {
            unreachable!()
        };
        element.font_family = "sans".into();
        element.font_size = 128.;
        element.outlined = true;
        element.text.clear();
        view.inline.as_mut().unwrap().text.clear();
        let frame = |view: &mut View, events: Vec<egui::Event>| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000., 900.),
                    )),
                    time: Some(if events.is_empty() { 0. } else { 1. }),
                    focused: true,
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        let area = ui.available_rect_before_wrap();
                        let preview = egui::Rect::from_min_size(area.min, egui::vec2(640., 360.));
                        show(ui, &tokens, view, area, preview);
                    });
                },
            )
        };
        frame(&mut view, vec![]).textures_delta.clear();
        let mut typed = frame(&mut view, vec![egui::Event::Text("Oo".into())]);
        typed.textures_delta.clear();
        assert_eq!(view.inline.as_ref().unwrap().text, "Oo");
        let frame = typed
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect) if rect.stroke.width == 1. => Some(rect.rect),
                _ => None,
            })
            .unwrap();
        let ink = typed
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Mesh(mesh) => Some(mesh.calc_bounds()),
                _ => None,
            })
            .reduce(|a, b| a.union(b))
            .unwrap();
        assert!(
            frame.contains_rect(ink),
            "input frame {frame:?} must contain ink {ink:?}"
        );
    }
}
