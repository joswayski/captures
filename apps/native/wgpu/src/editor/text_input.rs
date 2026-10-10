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
    pub(super) next_tool: Option<(Section, Option<DrawShape>)>,
    blocked: bool,
    focus: bool,
    ime_preedit: bool,
    rotated_drag: bool,
    #[cfg(target_os = "linux")]
    primary: Option<PrimaryPaste>,
    first_frame: Option<u64>,
    close_after: bool,
    previous_selection: Option<String>,
    previous_document: Arc<Document>,
    previous_output: Option<(egui::TextureHandle, u64)>,
    outline: outline::Outline,
}

#[cfg(target_os = "linux")]
struct PrimaryPaste {
    pending: crate::primary_selection::Pending,
    text: String,
    cursor: Option<egui::text::CCursorRange>,
    focus_epoch: u64,
    frame: u64,
}

#[cfg(target_os = "linux")]
fn focus_epoch(ctx: &egui::Context) -> u64 {
    ctx.data(|data| data.get_temp(egui::Id::unique(crate::clipboard_input::FOCUS_EPOCH)))
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
impl InlineText {
    pub(super) fn cancel_primary(&mut self) {
        self.primary.take();
    }
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
            next_tool: None,
            blocked: false,
            focus: true,
            ime_preedit: false,
            rotated_drag: false,
            #[cfg(target_os = "linux")]
            primary: None,
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
            #[cfg(target_os = "linux")]
            input.cancel_primary();
            if input.phase == Some(Phase::Finish) {
                return;
            }
            input.finish = Some(commit);
            if !commit {
                input.next_tool = None;
            }
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
            if let Some((section, shape)) = input.next_tool {
                self.activate_tool(section, shape);
            }
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
            if let Some((section, shape)) = input.next_tool {
                self.activate_tool(section, shape);
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
        input.next_tool = None;
        input.blocked = true;
        if std::mem::take(&mut input.close_after) {
            // Keep the failed composition available for retry, including after
            // the visible close deadline hid its viewport.
            self.cancel_close();
        }
    }

    pub(super) fn cancel_close(&mut self) {
        self.close_requested = false;
        self.close_deadline = None;
        self.close_after_save = false;
        if let Some(input) = &mut self.inline {
            input.close_after = false;
        }
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
/// Rotated labels paint their glyphs, plate, selection and caret rotated about
/// the frame centre. Pointer selection maps back to the unrotated galley while
/// TextEdit keeps keyboard/IME/clipboard ownership. Outlined labels replace
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
    #[cfg(target_os = "linux")]
    let mut primary_error = None;
    #[cfg(target_os = "linux")]
    let mut primary_gesture = None;
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
    let unrotate = |point: egui::Pos2| pivot + rotation.inverse() * (point - pivot);
    let rotate_rect = |rect: egui::Rect| {
        egui::Rect::from_points(&[
            rotate(rect.left_top()),
            rotate(rect.right_top()),
            rotate(rect.right_bottom()),
            rotate(rect.left_bottom()),
        ])
    };
    let hit_bounds = if rotated {
        rotate_rect(geometry.frame).union(geometry.frame)
    } else {
        geometry.frame
    };
    // An empty/boxed label's padding is wider than TextEdit's glyph area.
    // A Linux middle press belongs to the full inline frame, not click-away.
    let pointer_adapter = rotated
        || (cfg!(target_os = "linux")
            && ui.input(|i| {
                (i.pointer.button_pressed(egui::PointerButton::Middle)
                    || i.pointer.button_down(egui::PointerButton::Middle)
                    || i.pointer.button_released(egui::PointerButton::Middle))
                    && i.pointer.interact_pos().is_some_and(|pos| {
                        available.contains(pos) && geometry.frame.contains(unrotate(pos))
                    })
            }));
    let text_color = geometry.format.color;
    let separate_ink = rotated || geometry.outline_width.is_some();
    let mut lost_focus = false;
    let response = egui::Area::new(ui.scope_id().with((&input.id, "canvas-text-frame")))
        .order(egui::Order::Foreground)
        .fixed_pos(hit_bounds.min)
        .constrain(false)
        .show(ui.ctx(), |ui| {
            ui.set_clip_rect(available);
            if rotated {
                ui.set_min_size(hit_bounds.size());
            }
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
            // The pointer adapter is a separate widget. TextEdit must not blur
            // and collapse its anchor before handling same-frame keyboard input.
            // Actual click-away is handled below using the rotated frame.
            let focus_policy = pointer_adapter.then(|| {
                ui.ctx().options_mut(|options| {
                    std::mem::replace(
                        &mut options.input_options.surrender_focus_on,
                        egui::SurrenderFocusOn::Never,
                    )
                })
            });
            #[cfg(target_os = "linux")]
            let pasted = {
                let cursor = egui::text_edit::TextEditState::load(ui.ctx(), input_id)
                    .and_then(|state| state.cursor.char_range());
                if input.primary.as_ref().is_some_and(|paste| {
                    paste.text != input.text
                        || paste.cursor != cursor
                        || paste.focus_epoch != focus_epoch(ui.ctx())
                        || input.focus
                        || input.finish.is_some()
                        || finishing
                        || input.ime_preedit
                        || !ui.memory(|memory| memory.has_focus(input_id))
                        || ui.input(|i| {
                            !i.focused
                                || i.events.iter().any(|event| {
                                    matches!(
                                        event,
                                        egui::Event::Text(_)
                                            | egui::Event::Paste(_)
                                            | egui::Event::Cut
                                            | egui::Event::Ime(_)
                                            | egui::Event::WindowFocused(_)
                                            | egui::Event::Key { pressed: true, .. }
                                            | egui::Event::PointerButton { pressed: true, .. }
                                    )
                                })
                        })
                }) {
                    // Cancellation is monotonic: changing back or refocusing
                    // does not resurrect the old request.
                    input.primary.take();
                }
                if ui.ctx().current_pass_index() == 0
                    && let Some(paste) = &input.primary
                    && frame_nr > paste.frame
                    && let Some(result) = paste.pending.poll()
                {
                    input.primary.take();
                    match result {
                        Ok(text) if !text.is_empty() => {
                            ui.input_mut(|i| i.events.push(egui::Event::Paste(text)));
                            true
                        }
                        Ok(_) => false,
                        Err(error) => {
                            primary_error = Some(error);
                            false
                        }
                    }
                } else {
                    false
                }
            };
            let mut output = ui
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
            #[cfg(target_os = "linux")]
            if pasted {
                // No incoming Paste can coexist with delivery (guard above).
                // Only this TextEdit sees the temporary event, never another
                // field, another viewport, or a discarded layout pass.
                ui.input_mut(|i| {
                    i.events
                        .retain(|event| !matches!(event, egui::Event::Paste(_)))
                });
            }
            if let Some(policy) = focus_policy {
                ui.ctx()
                    .options_mut(|options| options.input_options.surrender_focus_on = policy);
            }
            let field = &output.response.response;
            // Only adapt metadata this field could have emitted, before the
            // pointer adapter or initial focus request changes IME ownership.
            let ime_caret =
                if rotated && !finishing && ui.memory(|memory| memory.owns_ime_events(input_id)) {
                    output
                        .state
                        .cursor
                        .range(&output.galley)
                        .map(|range| output.galley.pos_from_cursor(range.primary))
                } else {
                    None
                };
            let mut pointer_interacted = false;
            if pointer_adapter && !finishing {
                // This later response owns pointer hits instead of TextEdit's
                // axis-aligned box. Reject its empty corners, and keep only an
                // accepted primary gesture alive when dragging outside the box.
                let pointer_response = ui.interact(
                    hit_bounds,
                    input_id.with("rotated-pointer"),
                    egui::Sense::click_and_drag(),
                );
                if ui.ctx().current_pass_index() == 0 {
                    if let Some(pos) = pointer_response.interact_pointer_pos() {
                        let local = unrotate(pos);
                        let pressed_inside = ui.input(|i| i.pointer.any_pressed())
                            && pointer_response.hovered()
                            && available.contains(pos)
                            && geometry.frame.contains(local);
                        if ui.input(|i| i.pointer.primary_pressed()) {
                            input.rotated_drag = pressed_inside;
                        }
                        // TextEdit places its caret on any button press; only
                        // accepted primary gestures continue selection outside.
                        if input.rotated_drag || pressed_inside {
                            let origin =
                                output.galley_pos - egui::vec2(output.galley.rect.left(), 0.);
                            let cursor = output.galley.cursor_from_pos(local - origin);
                            pointer_interacted = output.state.cursor.pointer_interaction(
                                ui,
                                &pointer_response,
                                cursor,
                                &output.galley,
                                ui.input(|i| i.pointer.primary_down()),
                            );
                            if pointer_interacted {
                                output.cursor_range = output.state.cursor.range(&output.galley);
                                output.state.store(ui.ctx(), input_id);
                                field.request_focus();
                            }
                        }
                    }
                    input.rotated_drag &= ui.input(|i| i.pointer.primary_down());
                }
            }
            if let Some(previous_caret) = ime_caret {
                ui.output_mut(|platform| {
                    if let Some(ime) = &mut platform.ime {
                        if pointer_interacted && let Some(range) = output.cursor_range {
                            // TextEdit emitted before our pointer update. Retain
                            // its cursor padding/empty-row fallback, but move it
                            // to the same uniform-format galley caret we paint.
                            let caret = output.galley.pos_from_cursor(range.primary);
                            ime.cursor_rect =
                                ime.cursor_rect.translate(caret.min - previous_caret.min);
                        }
                        // egui-winit consumes rect; other integrations can use
                        // cursor_rect. Keep purpose and interruption unchanged.
                        ime.rect = rotate_rect(ime.rect);
                        ime.cursor_rect = rotate_rect(ime.cursor_rect);
                    }
                });
            }
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
            #[cfg(target_os = "linux")]
            {
                let cursor = egui::text_edit::TextEditState::load(ui.ctx(), input_id)
                    .and_then(|state| state.cursor.char_range());
                if input.primary.as_ref().is_some_and(|paste| {
                    paste.text != input.text || paste.cursor != cursor || !field.has_focus()
                }) {
                    input.primary.take();
                }
                if ui.ctx().current_pass_index() == 0
                    && !finishing
                    && input.finish.is_none()
                    && !input.ime_preedit
                    && field.has_focus()
                    && ui.input(|i| {
                        i.focused
                            && i.pointer.button_pressed(egui::PointerButton::Middle)
                            && i.pointer.interact_pos().is_some_and(|pos| {
                                available.contains(pos) && geometry.frame.contains(unrotate(pos))
                            })
                    })
                {
                    primary_gesture = Some(cursor);
                }
            }
            if field.changed() {
                input.blocked = false;
                // Geometry above used the buffer before TextEdit handled this
                // pass's events. Refit before presenting the new glyphs, not
                // after a later pointer event or cursor blink.
                ui.ctx().request_discard("inline typing refits its frame");
            }
            lost_focus = field.lost_focus() && !pointer_interacted;
            if field.has_focus() {
                // The accent outline is the indicator (`outline: 0` on the textarea).
                crate::primitives::focus_indicated(ui.ctx());
            }
            if separate_ink {
                let origin = output.galley_pos - egui::vec2(output.galley.rect.left(), 0.);
                if rotated
                    && field.has_focus()
                    && let Some(range) = output.cursor_range
                    && !range.is_empty()
                {
                    // Keep TextEdit's selection state, but paint its row backgrounds
                    // in the same coordinate space as our separate glyph ink.
                    let mut selected = output.galley.clone();
                    let mut visuals = ui.visuals().clone();
                    visuals.selection.bg_fill = tokens.color("theme-accent").gamma_multiply(0.2);
                    visuals.selection.stroke.color = egui::Color32::TRANSPARENT;
                    egui::text_selection::visuals::paint_text_selection(
                        &mut selected,
                        &visuals,
                        &range,
                        None,
                    );
                    let mut shape = egui::epaint::TextShape::new(
                        rotate(origin),
                        selected,
                        egui::Color32::TRANSPARENT,
                    );
                    // Only glyphs are overridden, not selection-background vertices.
                    // Outlined input must keep its hollow ink in the separate pass.
                    shape.override_text_color = Some(egui::Color32::TRANSPARENT);
                    shape.angle = geometry.angle;
                    painter.add(shape);
                }
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
    let clicked_elsewhere = if pointer_adapter {
        ui.input(|i| {
            i.pointer.any_pressed()
                && i.pointer.interact_pos().is_some_and(|pos| {
                    !available.contains(pos) || !geometry.frame.contains(unrotate(pos))
                })
        })
    } else {
        response.clicked_elsewhere()
    };
    let mut finish = None;
    if ui.ctx().current_pass_index() == 0 && !finishing && !first_frame {
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            if !ime_owned_escape {
                finish = Some(true);
            }
        } else if !blocked && (!ui.input(|i| i.focused) || clicked_elsewhere || lost_focus) {
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
    #[cfg(target_os = "linux")]
    {
        if let Some(error) = primary_error {
            view.error = Some(error);
        }
        if let Some(cursor) = primary_gesture
            && let Some(input) = &mut view.inline
            && input.finish.is_none()
            && !view.close_requested
            && !view.closed
            && let Some(reader) = ui.ctx().data(|data| {
                data.get_temp::<crate::primary_selection::Reader>(egui::Id::unique(
                    crate::primary_selection::ID,
                ))
            })
        {
            input.primary = Some(PrimaryPaste {
                pending: reader.request(ui.ctx().clone(), ui.ctx().viewport_id()),
                text: input.text.clone(),
                cursor,
                focus_epoch: focus_epoch(ui.ctx()),
                frame: frame_nr,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::presented_text;
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn primary_paste_delivers_once_at_the_placed_caret_and_cancels_stale_intent() {
        for angle in [0_f32, std::f32::consts::FRAC_PI_2, -0.53] {
            for case in [
                "deliver",
                "padding",
                "text",
                "caret",
                "focus",
                "native-focus",
                "ime",
                "finish",
                "close",
                "quit",
                "error",
            ] {
                if case == "padding" && angle != 0. {
                    continue;
                }
                let ctx = egui::Context::default();
                crate::ui_fonts::install(&ctx);
                let tokens = crate::tokens::load()["light-mustard"].clone();
                tokens.apply(&ctx, true);
                ctx.data_mut(|data| {
                    data.insert_temp(
                        egui::Id::unique(crate::primary_selection::ID),
                        crate::primary_selection::Reader::new(None),
                    )
                });
                let initial = if case == "padding" {
                    ""
                } else {
                    "One two\nalpha beta"
                };
                let mut presented = presented_text("label", initial);
                let document = Arc::make_mut(&mut presented.document);
                document.width = 640.;
                document.height = 360.;
                let Element::Text(element) = document.elements.last_mut().unwrap() else {
                    unreachable!()
                };
                element.base.x = 200.;
                element.base.y = 80.;
                element.base.rotation = Some(angle as f64);
                element.font_family = "mono".into();
                element.font_size = 30.;
                element.width = 220.;
                element.auto_width = Some(false);
                if case == "padding" {
                    element.auto_width = Some(true);
                    element.background = Some("#f7f7f5".into());
                }
                presented.pixels = Arc::new(RgbaImage::new(640, 360));
                let mut view = View::default();
                view.receive(&ctx, Ok(presented));
                let (tx, jobs) = mpsc::channel();
                view.begin_inline(&tx, TextInputTarget::Existing { id: "label".into() });
                assert!(matches!(
                    jobs.try_recv(),
                    Ok(Job::Apply(Request::BeginTextInput { .. }))
                ));
                let clock = std::cell::Cell::new(0.);
                let frame = |view: &mut View, events, discard| {
                    clock.set(clock.get() + 0.05);
                    let mut field = egui::Id::NULL;
                    let mut output = ctx.run_ui(egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(960., 700.))),
                        time: Some(clock.get()), focused: true, events, ..Default::default()
                    }, |ui| {
                        egui::CentralPanel::default().show(ui, |ui| {
                            field = ui.scope_id().with((&view.inline.as_ref().unwrap().id, "canvas-text-input"));
                            let available = ui.available_rect_before_wrap();
                            show(ui, &tokens, view, available, egui::Rect::from_min_size(egui::pos2(40., 30.), egui::vec2(640., 360.)));
                            assert!(!ui.input(|i| i.events.iter().any(|event| matches!(event, egui::Event::Paste(text) if text == "PRIMARY\n"))), "temporary paste leaked");
                        });
                        if discard && ctx.current_pass_index() == 0 { ctx.request_discard("PRIMARY multi-pass"); }
                    });
                    output.textures_delta.clear();
                    field
                };
                frame(&mut view, vec![], false);
                frame(&mut view, vec![], false);
                // Independent Mono boundary: index 2, pivot (310,118.55).
                let (dx, dy) = (236. - 310., 101. - 118.55);
                let pos = egui::pos2(
                    40. + 310. + angle.cos() * dx - angle.sin() * dy,
                    30. + 118.55 + angle.sin() * dx + angle.cos() * dy,
                );
                let pos = if case == "padding" {
                    egui::pos2(241., 111.)
                } else {
                    pos
                };
                frame(&mut view, vec![egui::Event::PointerMoved(pos)], false);
                let button = |pressed| egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Middle,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                };
                let field = frame(&mut view, vec![button(true)], true);
                assert_eq!(
                    view.inline.as_ref().unwrap().text,
                    initial,
                    "no paste on initiating pass"
                );
                let paste = view
                    .inline
                    .as_mut()
                    .unwrap()
                    .primary
                    .as_mut()
                    .expect("middle press starts a read");
                assert_eq!(
                    paste.cursor.unwrap().primary.index.0,
                    if case == "padding" { 0 } else { 2 },
                    "angle {angle}"
                );
                paste.pending = crate::primary_selection::Pending::ready(if case == "error" {
                    Err("unavailable fixture".into())
                } else {
                    Ok("PRIMARY\n".into())
                });
                let mut events = vec![button(false)];
                match case {
                    "text" => events.push(egui::Event::Text("X".into())),
                    "caret" => events.push(egui::Event::Key {
                        key: egui::Key::ArrowRight,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }),
                    "focus" => ctx.memory_mut(|memory| {
                        memory.surrender_focus(field);
                        memory.request_focus(egui::Id::unique("other field"));
                    }),
                    "native-focus" => ctx.data_mut(|data| {
                        data.insert_temp(
                            egui::Id::unique(crate::clipboard_input::FOCUS_EPOCH),
                            2_u64,
                        );
                    }),
                    "ime" => events.push(egui::Event::Ime(egui::ImeEvent::Commit("λ".into()))),
                    "finish" => view.finish_inline(true),
                    "close" => {
                        assert!(view.close_inline());
                    }
                    "quit" => view.inline.as_mut().unwrap().cancel_primary(),
                    _ => {}
                }
                frame(&mut view, events, true);
                let text = view.inline.as_ref().unwrap().text.clone();
                if case == "deliver" {
                    assert_eq!(text, "OnPRIMARY\ne two\nalpha beta");
                    assert!(view.inline.as_ref().unwrap().finish.is_none());
                } else if case == "padding" {
                    assert_eq!(text, "PRIMARY\n");
                    assert!(view.inline.as_ref().unwrap().finish.is_none());
                } else {
                    assert!(
                        !text.contains("PRIMARY"),
                        "stale {case}, angle {angle}: {text:?}"
                    );
                }
                assert!(view.inline.as_ref().unwrap().primary.is_none());
                if case == "error" {
                    assert_eq!(view.error.as_deref(), Some("unavailable fixture"));
                    assert_eq!(text, "One two\nalpha beta");
                }
                if case == "text" {
                    assert_eq!(text, "OnXe two\nalpha beta");
                    // Returning to the old contents must not revive delivery.
                    view.inline.as_mut().unwrap().text = "One two\nalpha beta".into();
                }
                frame(&mut view, vec![], true);
                if case != "text" {
                    assert_eq!(view.inline.as_ref().unwrap().text, text);
                }
                assert!(
                    jobs.try_recv().is_err(),
                    "paste remains local until normal worker drain"
                );
            }
        }
    }

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
    fn rail_click_during_text_preview_finishes_latest_buffer_and_retains_it_on_failure() {
        for fail in [false, true] {
            let (ctx, mut view, tx, rx) = setup();
            crate::ui_fonts::install(&ctx);
            let tokens = crate::tokens::load()["light-mustard"].clone();
            tokens.apply(&ctx, true);
            view.section = Section::Draw;
            view.draw_shape = DrawShape::Text;
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
                    |ui| super::super::show(ui, &tokens, view, &tx),
                );
                output.textures_delta.clear();
            };
            frame(&mut view, vec![]);
            frame(&mut view, vec![egui::Event::Text(" latest".into())]);
            assert_eq!(view.inline.as_ref().unwrap().text, "original latest");
            // Arrow's shipping rail position, while Begin is still rendering.
            let pos = egui::pos2(28., 239.);
            for pressed in [true, false] {
                frame(
                    &mut view,
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
            }
            assert_eq!(
                view.draw_shape,
                DrawShape::Text,
                "Finish must accept before switching"
            );
            accept(&ctx, &mut view, "original");
            view.drain_inline(&tx);
            assert!(
                matches!(rx.try_recv(), Ok(Job::Apply(Request::UpdateTextInput { text, .. }))
                if text == "original latest")
            );
            accept(&ctx, &mut view, "original latest");
            view.drain_inline(&tx);
            assert!(matches!(
                rx.try_recv(),
                Ok(Job::Apply(Request::FinishTextInput { commit: true, .. }))
            ));
            if fail {
                view.receive(&ctx, Err("finish unavailable".into()));
                assert_eq!(view.draw_shape, DrawShape::Text);
                assert_eq!(view.inline.as_ref().unwrap().text, "original latest");
                view.drain_inline(&tx);
                assert!(
                    rx.try_recv().is_err(),
                    "failed Finish must not retry automatically"
                );
                // A later explicit text finish must not revive the failed tool choice.
                view.finish_inline(true);
                view.drain_inline(&tx);
                assert!(matches!(
                    rx.try_recv(),
                    Ok(Job::Apply(Request::FinishTextInput { .. }))
                ));
            }
            view.receive(&ctx, Ok(presented_text("label", "original latest")));
            assert!(view.inline.is_none());
            assert_eq!(
                view.draw_shape,
                if fail {
                    DrawShape::Text
                } else {
                    DrawShape::Arrow
                }
            );
            assert_eq!(view.selected_layer.is_none(), !fail);
        }
    }

    #[test]
    fn inline_ime_placement_rotates_current_field_and_caret_without_changing_composition() {
        for (angle, scale) in [
            (std::f32::consts::FRAC_PI_2, 1_f32),
            (-std::f32::consts::FRAC_PI_2, 0.75),
            (0.61, 1.3),
            (-0.37, 0.9),
        ] {
            let run = |angle: f32| {
                let ctx = egui::Context::default();
                crate::ui_fonts::install(&ctx);
                let tokens = crate::tokens::load()["light-mustard"].clone();
                tokens.apply(&ctx, true);
                let mut presented = presented_text("label", "One two\nalpha beta");
                let document = Arc::make_mut(&mut presented.document);
                document.width = 640.;
                document.height = 360.;
                let Element::Text(element) = document.elements.last_mut().unwrap() else {
                    unreachable!()
                };
                element.base.x = 200.;
                element.base.y = 80.;
                element.base.rotation = Some(angle as f64);
                element.font_family = "mono".into();
                element.font_size = 30.;
                element.width = 220.;
                element.auto_width = Some(false);
                element.outlined = true;
                presented.pixels = Arc::new(RgbaImage::new(640, 360));
                let mut view = View::default();
                view.receive(&ctx, Ok(presented));
                let (tx, jobs) = mpsc::channel();
                view.begin_inline(&tx, TextInputTarget::Existing { id: "label".into() });
                assert!(matches!(
                    jobs.try_recv(),
                    Ok(Job::Apply(Request::BeginTextInput { .. }))
                ));
                let child = egui::ViewportId::from_hash_of("rotated-ime-editor");
                let clock = std::cell::Cell::new(0_f64);
                let force_discard = std::cell::Cell::new(true);
                let frame = |view: &mut View, events| {
                    clock.set(clock.get() + 1.);
                    let mut raw = egui::RawInput {
                        viewport_id: child,
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(960., 700.),
                        )),
                        time: Some(clock.get()),
                        focused: true,
                        events,
                        ..Default::default()
                    };
                    raw.viewports.insert(
                        child,
                        egui::ViewportInfo {
                            parent: Some(egui::ViewportId::ROOT),
                            ..Default::default()
                        },
                    );
                    let mut output = ctx.run_ui(raw, |ui| {
                        assert_eq!(ui.ctx().viewport_id(), child);
                        egui::CentralPanel::default().show(ui, |ui| {
                            show(
                                ui,
                                &tokens,
                                view,
                                ui.available_rect_before_wrap(),
                                egui::Rect::from_min_size(
                                    egui::pos2(40., 30.),
                                    egui::vec2(640., 360.) * scale,
                                ),
                            );
                        });
                        if force_discard.get() && ctx.current_pass_index() == 0 {
                            ctx.request_discard("IME refit multi-pass");
                        }
                    });
                    output.textures_delta.clear();
                    if force_discard.get() {
                        assert_eq!(output.platform_output.num_completed_passes, 2);
                    } else {
                        assert_eq!(output.platform_output.num_completed_passes, 1);
                    }
                    (
                        output.platform_output.ime,
                        view.inline.as_ref().unwrap().text.split('\n').count(),
                    )
                };
                frame(&mut view, vec![]);
                frame(&mut view, vec![]);
                // Independent mono cursor 2 on row one, around the asymmetric
                // 220 × (75 + 2.1) frame's centre, not around the content origin.
                let point = egui::pos2(
                    40. + (310. + angle.cos() * -74. - angle.sin() * -17.55) * scale,
                    30. + (118.55 + angle.sin() * -74. + angle.cos() * -17.55) * scale,
                );
                let button = |pressed| egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                };
                frame(&mut view, vec![egui::Event::PointerMoved(point)]);
                // Pointer metadata must be current even without another pass.
                force_discard.set(false);
                let pressed = frame(&mut view, vec![button(true)]);
                let mut outputs = vec![(pressed.0.unwrap(), pressed.1)];
                let released = frame(&mut view, vec![button(false)]);
                outputs.push((released.0.unwrap(), released.1));
                force_discard.set(true);
                for events in [
                    vec![egui::Event::Ime(egui::ImeEvent::Preedit {
                        text: "候補".into(),
                        active_range_chars: None,
                    })],
                    vec![egui::Event::Ime(egui::ImeEvent::Commit("漢".into()))],
                    vec![egui::Event::Text("\nthird".into())],
                ] {
                    let output = frame(&mut view, events);
                    outputs.push((output.0.unwrap(), output.1));
                }
                assert!(jobs.try_recv().is_err());
                assert!(view.inline.as_ref().unwrap().finish.is_none());
                (outputs, view.inline.unwrap().text)
            };
            let (plain, plain_text) = run(0.);
            let (turned, turned_text) = run(angle);
            assert_eq!(
                turned_text, plain_text,
                "composition semantics stay in TextEdit"
            );
            assert_eq!(plain_text, "On漢\nthirde two\nalpha beta");
            assert_eq!(plain[0].1, 2);
            assert_eq!(plain.last().unwrap().1, 3);
            for ((plain, rows), (turned, turned_rows)) in plain.iter().zip(&turned) {
                assert_eq!(rows, turned_rows);
                // Derive AABB centre/extents in closed form rather than using
                // the implementation's four-corner transform. The frame is
                // 220px wide, each row is 37.5px tall, and optical top padding
                // is 7% of the 30px type. TextEdit's atom is not the whole frame.
                let pivot = egui::pos2(
                    40. + 310. * scale,
                    30. + (80. + (37.5 * *rows as f32 + 2.1) / 2.) * scale,
                );
                let expected = |rect: egui::Rect| {
                    let offset = rect.center() - pivot;
                    let center = pivot
                        + egui::vec2(
                            angle.cos() * offset.x - angle.sin() * offset.y,
                            angle.sin() * offset.x + angle.cos() * offset.y,
                        );
                    let size = egui::vec2(
                        angle.cos().abs() * rect.width() + angle.sin().abs() * rect.height(),
                        angle.sin().abs() * rect.width() + angle.cos().abs() * rect.height(),
                    );
                    egui::Rect::from_center_size(center, size)
                };
                for (actual, expected) in [
                    (turned.rect, expected(plain.rect)),
                    (turned.cursor_rect, expected(plain.cursor_rect)),
                ] {
                    assert!(
                        actual.min.distance(expected.min) < 0.02
                            && actual.max.distance(expected.max) < 0.02,
                        "angle {angle}, scale {scale}: expected {expected:?}, got {actual:?}"
                    );
                }
                assert_eq!(turned.purpose, plain.purpose);
                assert_eq!(
                    turned.should_interrupt_composition,
                    plain.should_interrupt_composition
                );
            }
        }
    }

    #[test]
    fn inline_ime_adapter_does_not_rotate_another_fields_output_or_a_finishing_field() {
        let (ctx, mut view, _, _) = setup();
        crate::ui_fonts::install(&ctx);
        let tokens = crate::tokens::load()["light-mustard"].clone();
        tokens.apply(&ctx, true);
        let Element::Text(element) = Arc::make_mut(&mut view.presented.as_mut().unwrap().document)
            .elements
            .last_mut()
            .unwrap()
        else {
            unreachable!()
        };
        element.base.rotation = Some(0.61);
        let mut password = "other input".to_owned();
        for (frame, finishing) in [false, false, false, true].into_iter().enumerate() {
            if finishing {
                view.inline.as_mut().unwrap().phase = Some(Phase::Finish);
            }
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(960., 700.),
                    )),
                    focused: true,
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        let other = egui::TextEdit::singleline(&mut password)
                            .id_source("other-ime-owner")
                            .password(true)
                            .show(ui);
                        other.response.response.request_focus();
                        let mut before = ui.ctx().output(|output| output.ime);
                        if frame == 2 {
                            assert_eq!(before.unwrap().purpose, egui::IMEPurpose::Password);
                        }
                        if finishing {
                            let id = ui
                                .scope_id()
                                .with((&view.inline.as_ref().unwrap().id, "canvas-text-input"));
                            ui.memory_mut(|memory| memory.request_focus(id));
                            // A finishing, noninteractive field must leave even
                            // existing output untouched, despite retained focus.
                            before = Some(egui::output::IMEOutput {
                                purpose: egui::IMEPurpose::Password,
                                rect: egui::Rect::from_min_size(
                                    egui::pos2(71., 29.),
                                    egui::vec2(91., 23.),
                                ),
                                cursor_rect: egui::Rect::from_min_size(
                                    egui::pos2(111., 31.),
                                    egui::vec2(3., 19.),
                                ),
                                should_interrupt_composition: true,
                            });
                            ui.output_mut(|output| output.ime = before);
                        }
                        show(
                            ui,
                            &tokens,
                            &mut view,
                            ui.available_rect_before_wrap(),
                            egui::Rect::from_min_size(egui::pos2(40., 70.), egui::vec2(640., 360.)),
                        );
                        assert_eq!(ui.ctx().output(|output| output.ime), before);
                    });
                },
            );
            output.textures_delta.clear();
            if let Some(ime) = output.platform_output.ime {
                assert_eq!(ime.purpose, egui::IMEPurpose::Password);
            }
            view.inline.as_mut().unwrap().focus = false;
        }
    }

    #[test]
    fn inline_pointer_selection_uses_rotated_galley_coordinates_and_keeps_text_focus() {
        for (angle, scale, outlined) in [
            (0_f32, 1_f32, false),
            (std::f32::consts::FRAC_PI_2, 1., false),
            (-0.53, 0.75, true),
            (0.64, 1.3, false),
        ] {
            let ctx = egui::Context::default();
            crate::ui_fonts::install(&ctx);
            let tokens = crate::tokens::load()["light-mustard"].clone();
            tokens.apply(&ctx, true);
            ctx.options_mut(|options| {
                options.input_options.surrender_focus_on = egui::SurrenderFocusOn::Presses
            });
            let mut presented = presented_text("label", "One two\nalpha beta");
            let document = Arc::make_mut(&mut presented.document);
            document.width = 640.;
            document.height = 360.;
            let Element::Text(element) = document.elements.last_mut().unwrap() else {
                unreachable!()
            };
            element.base.x = 200.;
            element.base.y = 80.;
            element.base.rotation = Some(angle as f64);
            element.font_family = "mono".into();
            element.font_size = 30.;
            element.width = 220.;
            element.auto_width = Some(false);
            element.outlined = outlined;
            presented.pixels = Arc::new(RgbaImage::new(640, 360));
            let mut view = View::default();
            view.receive(&ctx, Ok(presented));
            let (tx, jobs) = mpsc::channel();
            view.begin_inline(&tx, TextInputTarget::Existing { id: "label".into() });
            assert!(matches!(
                jobs.try_recv(),
                Ok(Job::Apply(Request::BeginTextInput { .. }))
            ));
            let clock = std::cell::Cell::new(0_f64);
            let input_field = std::cell::Cell::new(egui::Id::NULL);
            let frame = |view: &mut View, mut events: Vec<egui::Event>| {
                clock.set(clock.get() + 0.05);
                let modifiers = events
                    .iter()
                    .find_map(|event| match event {
                        egui::Event::PointerButton { modifiers, .. } => Some(*modifiers),
                        _ => None,
                    })
                    .unwrap_or_default();
                events.insert(0, egui::Event::ModifiersChanged(modifiers));
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(960., 700.),
                        )),
                        time: Some(clock.get()),
                        focused: true,
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        egui::CentralPanel::default().show(ui, |ui| {
                            let area = ui.available_rect_before_wrap();
                            input_field.set(
                                ui.scope_id()
                                    .with((&view.inline.as_ref().unwrap().id, "canvas-text-input")),
                            );
                            show(
                                ui,
                                &tokens,
                                view,
                                area,
                                egui::Rect::from_min_size(
                                    egui::pos2(40., 30.),
                                    egui::vec2(640., 360.) * scale,
                                ),
                            );
                        });
                        if ctx.current_pass_index() == 0 {
                            ctx.request_discard("pointer selection multi-pass");
                        }
                    },
                );
                output.textures_delta.clear();
                output
            };
            // Independent 220px frame, two 37.5px rows and 2.1px optical padding.
            // Liberation Mono's 30px ASCII advance is 18px; use cursor boundaries,
            // not the implementation's galley cursor positions, as pointer inputs.
            let pointer = |x: f32, y: f32| {
                let (dx, dy) = (x - 310., y - 118.55);
                egui::pos2(
                    40. + (310. + angle.cos() * dx - angle.sin() * dy) * scale,
                    30. + (118.55 + angle.sin() * dx + angle.cos() * dy) * scale,
                )
            };
            let button = |pos, pressed, modifiers| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers,
            };
            let fast_click = |view: &mut View, pos, modifiers| {
                frame(view, vec![egui::Event::PointerMoved(pos)]);
                frame(view, vec![button(pos, true, modifiers)]);
                frame(view, vec![button(pos, false, modifiers)]);
            };
            let click = |view: &mut View, pos, modifiers| {
                clock.set(clock.get() + 1.);
                fast_click(view, pos, modifiers);
            };
            let copy = |view: &mut View, expected: &str| {
                let output = frame(view, vec![egui::Event::Copy]);
                assert!(
                    output
                        .platform_output
                        .commands
                        .iter()
                        .any(|command| matches!(
                            command, egui::OutputCommand::CopyText(text) if text == expected
                        )),
                    "angle {angle}, scale {scale}: expected selection {expected:?}, commands {:?}",
                    output.platform_output.commands
                );
            };
            frame(&mut view, vec![]);
            frame(&mut view, vec![]);
            click(&mut view, pointer(236., 101.), egui::Modifiers::NONE);
            assert!(
                view.inline.as_ref().unwrap().finish.is_none(),
                "a rotated click is not click-away"
            );
            assert!(ctx.text_edit_focused());
            click(&mut view, pointer(290., 138.), egui::Modifiers::SHIFT);
            assert!(
                ctx.text_edit_focused(),
                "Shift-click keeps text focus: angle {angle}, finish {:?}",
                view.inline.as_ref().unwrap().finish
            );
            copy(&mut view, "e two\nalpha");
            // The accepted drag owns selection even beyond the rotated frame.
            let start = pointer(200., 138.);
            frame(&mut view, vec![egui::Event::PointerMoved(start)]);
            frame(&mut view, vec![button(start, true, egui::Modifiers::NONE)]);
            let end = pointer(560., 138.);
            frame(&mut view, vec![egui::Event::PointerMoved(end)]);
            frame(&mut view, vec![button(end, false, egui::Modifiers::NONE)]);
            copy(&mut view, "alpha beta");
            assert!(
                view.inline.as_ref().unwrap().finish.is_none(),
                "dragging outside does not commit"
            );
            let word = pointer(254., 138.);
            click(&mut view, word, egui::Modifiers::NONE);
            fast_click(&mut view, word, egui::Modifiers::NONE);
            copy(&mut view, "alpha");
            fast_click(&mut view, word, egui::Modifiers::NONE);
            copy(&mut view, "alpha beta");
            // TextEdit places the caret on any button press, not just primary
            // clicks. Match that on turned text without adding nonprimary drag
            // selection or pasting from a different clipboard selection.
            for pointer_button in [egui::PointerButton::Middle, egui::PointerButton::Secondary] {
                clock.set(clock.get() + 1.);
                let start = pointer(236., 101.);
                let nonprimary = |pos, pressed, modifiers| egui::Event::PointerButton {
                    pos,
                    button: pointer_button,
                    pressed,
                    modifiers,
                };
                frame(&mut view, vec![egui::Event::PointerMoved(start)]);
                frame(
                    &mut view,
                    vec![nonprimary(start, true, egui::Modifiers::NONE)],
                );
                let range = egui::text_edit::TextEditState::load(&ctx, input_field.get())
                    .unwrap()
                    .cursor
                    .char_range()
                    .unwrap();
                assert_eq!(
                    (range.primary.index.0, range.secondary.index.0),
                    (2, 2),
                    "press {pointer_button:?}, angle {angle}, focus {}, finish {:?}",
                    ctx.text_edit_focused(),
                    view.inline.as_ref().unwrap().finish
                );
                assert!(!view.inline.as_ref().unwrap().rotated_drag);
                frame(
                    &mut view,
                    vec![nonprimary(start, false, egui::Modifiers::NONE)],
                );
                let end = pointer(290., 138.);
                frame(&mut view, vec![egui::Event::PointerMoved(end)]);
                frame(
                    &mut view,
                    vec![nonprimary(end, true, egui::Modifiers::SHIFT)],
                );
                let range = egui::text_edit::TextEditState::load(&ctx, input_field.get())
                    .unwrap()
                    .cursor
                    .char_range()
                    .unwrap();
                assert_eq!((range.primary.index.0, range.secondary.index.0), (13, 2));
                frame(
                    &mut view,
                    vec![nonprimary(end, false, egui::Modifiers::SHIFT)],
                );
                copy(&mut view, "e two\nalpha");
                assert!(ctx.text_edit_focused());
                assert!(view.inline.as_ref().unwrap().finish.is_none());
            }
            assert_eq!(view.inline.as_ref().unwrap().text, "One two\nalpha beta");
            assert!(
                jobs.try_recv().is_err(),
                "pointer selection sends no document jobs"
            );
            assert!(ctx.text_edit_focused());
            // A release can share a frame with typing. Do not lose that event
            // to temporary blur, or move the next typed character before it.
            clock.set(clock.get() + 1.);
            frame(&mut view, vec![egui::Event::PointerMoved(word)]);
            frame(&mut view, vec![button(word, true, egui::Modifiers::NONE)]);
            frame(
                &mut view,
                vec![
                    button(word, false, egui::Modifiers::NONE),
                    egui::Event::Text("!".into()),
                ],
            );
            frame(&mut view, vec![egui::Event::Text("?".into())]);
            assert_eq!(view.inline.as_ref().unwrap().text, "One two\nalp!?ha beta");
            assert_eq!(
                ctx.options(|options| options.input_options.surrender_focus_on),
                egui::SurrenderFocusOn::Presses
            );
            if angle != 0. && angle != std::f32::consts::FRAC_PI_2 {
                let bounds = egui::Rect::from_points(&[
                    pointer(200., 80.),
                    pointer(420., 80.),
                    pointer(420., 157.1),
                    pointer(200., 157.1),
                ]);
                click(
                    &mut view,
                    bounds.min + egui::vec2(1., 1.),
                    egui::Modifiers::NONE,
                );
                assert_eq!(
                    view.inline.as_ref().unwrap().finish,
                    Some(true),
                    "an empty AABB corner is click-away"
                );
            }
        }
    }

    #[test]
    fn selected_inline_rows_rotate_with_the_ink_without_changing_the_buffer() {
        for (theme, angle, outlined, scale) in [
            ("light-mustard", 0., false, 1.),
            ("dark-mustard", std::f32::consts::FRAC_PI_2, false, 1.),
            ("light-mustard", -0.43, false, 0.65),
            ("dark-mustard", 0.71, true, 1.3),
        ] {
            let ctx = egui::Context::default();
            let tokens = crate::tokens::load()[theme].clone();
            tokens.apply(&ctx, true);
            crate::ui_fonts::install(&ctx);
            let mut presented = presented_text("label", "Wide ABCD\nhi");
            let document = Arc::make_mut(&mut presented.document);
            document.width = 640.;
            document.height = 360.;
            let Element::Text(element) = document.elements.last_mut().unwrap() else {
                unreachable!()
            };
            element.base.x = 200.;
            element.base.y = 80.;
            element.base.rotation = Some(angle as f64);
            element.font_family = "sans".into();
            element.width = 220.;
            element.auto_width = Some(false);
            element.outlined = outlined;
            presented.pixels = Arc::new(RgbaImage::new(640, 360));
            let mut view = View::default();
            view.receive(&ctx, Ok(presented));
            let (tx, jobs) = mpsc::channel();
            view.begin_inline(&tx, TextInputTarget::Existing { id: "label".into() });
            assert!(matches!(
                jobs.try_recv(),
                Ok(Job::Apply(Request::BeginTextInput { .. }))
            ));
            let frame = |view: &mut View, events| {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(960., 700.),
                        )),
                        time: Some(ctx.cumulative_frame_nr() as f64),
                        focused: true,
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        egui::CentralPanel::default().show(ui, |ui| {
                            let area = ui.available_rect_before_wrap();
                            let preview = egui::Rect::from_min_size(
                                egui::pos2(40., 30.),
                                egui::vec2(640., 360.) * scale,
                            );
                            show(ui, &tokens, view, area, preview);
                        });
                    },
                );
                output.textures_delta.clear();
                output
            };
            let key = |key, modifiers| egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            };
            frame(&mut view, vec![]);
            frame(&mut view, vec![]);
            let output = frame(
                &mut view,
                vec![
                    key(egui::Key::A, egui::Modifiers::COMMAND),
                    egui::Event::Copy,
                ],
            );
            assert!(
                output
                    .platform_output
                    .commands
                    .iter()
                    .any(|command| matches!(
                        command, egui::OutputCommand::CopyText(text) if text == "Wide ABCD\nhi"
                    ))
            );
            let highlight = tokens.color("theme-accent").gamma_multiply(0.2);
            let points: Vec<_> = ctx
                .tessellate(output.shapes, output.pixels_per_point)
                .into_iter()
                .flat_map(|clipped| match clipped.primitive {
                    egui::epaint::Primitive::Mesh(mesh) => mesh.vertices,
                    _ => vec![],
                })
                .filter(|vertex| vertex.color == highlight)
                .map(|vertex| vertex.pos)
                .collect();
            assert_eq!(
                points.len(),
                8,
                "two visible selected rows: {theme}, {angle}, {outlined}"
            );
            let mut widths = vec![];
            for quad in points.chunks_exact(4) {
                // Inverse rotation must recover an axis-aligned rectangle.
                // The 220px frame contains two 40px rows plus the 32px font's
                // 7% optical top padding. Compute its pivot independently.
                let pivot = egui::pos2(
                    40. + 310. * scale,
                    30. + (80. + (80. + 32. * 0.07) / 2.) * scale,
                );
                let local: Vec<_> = quad
                    .iter()
                    .map(|point| {
                        let offset = *point - pivot;
                        egui::pos2(
                            angle.cos() * offset.x + angle.sin() * offset.y,
                            -angle.sin() * offset.x + angle.cos() * offset.y,
                        ) + pivot.to_vec2()
                    })
                    .collect();
                let bounds = egui::Rect::from_points(&local);
                assert!(
                    (bounds.left() - (40. + 200. * scale)).abs() < 1.,
                    "selection must share the glyph origin, not just its angle: {bounds:?}"
                );
                for point in local {
                    assert!(
                        (point.x - bounds.left()).abs() < 0.01
                            || (point.x - bounds.right()).abs() < 0.01
                    );
                    assert!(
                        (point.y - bounds.top()).abs() < 0.01
                            || (point.y - bounds.bottom()).abs() < 0.01
                    );
                }
                widths.push(bounds.width());
            }
            assert!(
                widths[0] > widths[1] * 2.,
                "selection follows asymmetric rows, not the whole frame"
            );
            let cleared = frame(
                &mut view,
                vec![key(egui::Key::End, egui::Modifiers::COMMAND)],
            );
            assert!(
                !ctx.tessellate(cleared.shapes, cleared.pixels_per_point)
                    .iter()
                    .any(|clipped| {
                        matches!(&clipped.primitive, egui::epaint::Primitive::Mesh(mesh)
                    if mesh.vertices.iter().any(|vertex| vertex.color == highlight))
                    }),
                "collapsing the range removes the highlight"
            );
            assert_eq!(view.inline.as_ref().unwrap().text, "Wide ABCD\nhi");
            assert!(
                jobs.try_recv().is_err(),
                "selection does not edit or finish the document"
            );
        }
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
        assert!(
            view.close_deadline.is_none(),
            "a failed finish restores the window for retry"
        );
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
        assert!(!view.closed && view.close_requested);
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
