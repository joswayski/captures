//! One local typing buffer and at most one accepted worker operation. Input stays
//! responsive while shared Rust fits/renders; only Finish publishes document history.
use super::*;
use captures_app::editor_session::TextInputTarget;

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
    anchor: Point,
    phase: Option<Phase>,
    finish: Option<bool>,
    blocked: bool,
    focus: bool,
    ime_preedit: bool,
    first_frame: Option<u64>,
    close_after: bool,
    previous_selection: Option<String>,
    previous_document: Arc<Document>,
    previous_output: Option<(egui::TextureHandle, usize)>,
    previous_show_output: bool,
}

impl View {
    pub(super) fn begin_inline(
        &mut self,
        tx: &Sender<Job>,
        target: TextInputTarget,
        anchor: Point,
    ) {
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
            anchor,
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
            previous_show_output: self.show_output,
        });
        self.cancel_edit_gestures();
        self.cancel_crop();
        self.viewport_pan = None;
        self.show_output = false;
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
        if input.finish == Some(false) && !input.started {
            let input = self.inline.take().unwrap();
            self.output = input.previous_output;
            self.show_output = input.previous_show_output;
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
                self.show_output = input.previous_show_output;
                self.select_layer_exact(input.previous_selection);
            }
            self.activate_tool(Section::Layers, None);
            if input.close_after {
                self.request_close();
            }
        }
    }

    pub(super) fn inline_failed(&mut self) {
        let Some(input) = &mut self.inline else {
            return;
        };
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

pub(super) fn show(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    available: egui::Rect,
    preview: egui::Rect,
) {
    let Some(input) = &mut view.inline else {
        return;
    };
    // This first native input surface is unrotated and uses the UI font. The
    // underlying accepted preview remains the pinned-font, styled/rotated render.
    // Never pretend the native text control supplies document layout metrics.
    let scale = preview.width() / view.canvas[0] as f32;
    let anchor = preview.min + egui::vec2(input.anchor.x as f32, input.anchor.y as f32) * scale;
    let width = (tokens.number("s-12") * 5.).min(available.width());
    let height = tokens.number("s-12") * 3.;
    let position = egui::pos2(
        anchor.x.clamp(
            available.left(),
            (available.right() - width).max(available.left()),
        ),
        anchor.y.clamp(
            available.top(),
            (available.bottom() - height).max(available.top()),
        ),
    );
    let input_id = ui.scope_id().with((&input.id, "canvas-text-input"));
    let finishing = input.phase == Some(Phase::Finish);
    let frame = ui.ctx().cumulative_frame_nr();
    let first_frame = *input.first_frame.get_or_insert(frame) == frame;
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
    let mut finish = None;
    let response = egui::Area::new(ui.scope_id().with((&input.id, "canvas-text-frame")))
        .order(egui::Order::Foreground)
        .fixed_pos(position)
        .constrain_to(available)
        .show(ui.ctx(), |ui| {
            ui.set_width(width);
            egui::Frame::new()
                .fill(tokens.color("surface-raised"))
                .stroke(egui::Stroke::new(1., tokens.color("theme-accent")))
                .inner_margin(tokens.number("s-2"))
                .show(ui, |ui| {
                    let label = ui.label("Text input");
                    let field = egui::ScrollArea::vertical()
                        .max_height(tokens.number("s-12") * 2.)
                        .show(ui, |ui| {
                            ui.add_enabled(
                                !finishing,
                                egui::TextEdit::multiline(&mut input.text)
                                    .id(input_id)
                                    // Keep focus until earlier queued edits have run.
                                    // The host handles Escape after the field below.
                                    .event_filter(egui::EventFilter {
                                        horizontal_arrows: true,
                                        vertical_arrows: true,
                                        escape: true,
                                        ..Default::default()
                                    })
                                    .desired_width(width)
                                    .desired_rows(3),
                            )
                            .labelled_by(label.id)
                        })
                        .inner;
                    if input.focus {
                        field.request_focus();
                        input.focus = false;
                    }
                    if field.changed() {
                        input.blocked = false;
                    }
                    ui.add_enabled_ui(!finishing, |ui| {
                        ui.horizontal(|ui| {
                            if ui
                                .button(if input.blocked { "Retry" } else { "Done" })
                                .clicked()
                            {
                                finish = Some(true);
                            }
                            if ui.button("Cancel").clicked() {
                                finish = Some(false);
                            }
                        });
                    });
                    ui.small("Enter: new line · Escape: finish");
                });
        })
        .response;
    if ui.ctx().current_pass_index() == 0 && !finishing && !first_frame {
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            if !ime_owned_escape {
                finish = Some(true);
            }
        } else if !blocked && (!ui.input(|i| i.focused) || response.clicked_elsewhere()) {
            finish.get_or_insert(true);
        }
    }
    if let Some(commit) = finish {
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
        view.show_output = true;
        let (tx, rx) = mpsc::channel();
        view.begin_inline(
            &tx,
            TextInputTarget::Existing { id: "label".into() },
            Point { x: 2., y: 1. },
        );
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
        assert!(view.inline.is_none() && view.show_output);
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
        assert!(view.inline.is_none() && view.show_output && rx.try_recv().is_err());

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
                    },
                },
                Point { x: 23., y: 31. },
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
}
