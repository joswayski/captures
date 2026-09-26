use captures_app::preview_chrome::{self, EditorPhase};
use eframe::egui::{self, Color32, Stroke};

use crate::tokens::Tokens;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    ExpandStack,
    MoveStack(egui::Pos2),
    DragFile,
    Copy,
    Save,
    Reveal,
    Edit,
    Trash,
    Dismiss,
    /// An unsaved card's Delete: the preview dissolves; History keeps it.
    Discard,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StackAction {
    ToggleCollapsed,
    ClearAll,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Busy {
    Drag,
    Copy,
    Save,
    Reveal,
    Trash,
}

pub fn stack_controls_visible(count: usize, collapsed: bool) -> bool {
    !collapsed && count >= 2
}

pub struct View<'a> {
    pub artifact_id: &'a str,
    pub texture: &'a egui::TextureHandle,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
    pub busy: Option<Busy>,
    pub message: Option<&'a str>,
    /// The clipboard still holds this capture: hide Copy, show the chip.
    pub clipboard_current: bool,
    /// Brief "Saved" confirmation after an explicit save.
    pub saved_feedback: bool,
    pub can_save: bool,
    pub saved: bool,
    pub interactive: bool,
    pub collapsed: bool,
    pub stack_count: usize,
    pub depth: usize,
    pub desktop_pointer: Option<egui::Pos2>,
    pub reject_offset: f32,
    pub right_anchor: bool,
    pub top_anchor: bool,
    /// Card-sized pre-blurred copy of the media for the hover treatment.
    pub blurred: Option<&'a egui::TextureHandle>,
    /// Shared editor presence phase for this capture.
    pub editor: EditorPhase,
    /// Milliseconds since `editor` began (ring and pill transitions).
    pub editor_elapsed_ms: f64,
    /// Shipping stale-pointer lock: hover chrome stays idle until the
    /// pointer moves after an expand or a new capture.
    pub hover_locked: bool,
    pub reduced_motion: bool,
    /// Opacity of the `thumbnail-capture-highlight` accent outline.
    pub highlight: f32,
    /// Shipping warning chip beside the metadata ("Not in History",
    /// "Clipboard unavailable").
    pub warning: Option<&'a str>,
    /// Multiplier on a compact card's depth shade while the stack flies
    /// between the list and the pile (1 at rest).
    pub depth_shade: f32,
}

pub fn reject_offset(elapsed_seconds: f32, reduced_motion: bool) -> f32 {
    let duration = captures_app::preview::PREVIEW_DROP_REJECT_MS as f32 / 1000.;
    if reduced_motion || elapsed_seconds >= duration {
        return 0.;
    }
    let progress = (elapsed_seconds / duration).clamp(0., 1.);
    let stops = [0., -8., 7., -5., 3., 0.];
    let position = progress * 5.;
    let index = (position as usize).min(4);
    egui::lerp(stops[index]..=stops[index + 1], position - index as f32)
}

pub fn show(ui: &mut egui::Ui, tokens: &Tokens, view: View<'_>) -> Option<Action> {
    let mut action = None;
    let size = egui::vec2(
        ui.available_width(),
        captures_app::preview::THUMBNAIL_CARD_HEIGHT as f32,
    );
    let (card, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let card = card.translate(egui::vec2(view.reject_offset, 0.));
    let radius = tokens.number("thumbnail-card-radius");

    if view.collapsed {
        ui.painter()
            .rect_filled(card, radius, tokens.color("glass-raised"));
        paint_media(ui, card, radius, view.texture, None, 0., 0.);
        if view.depth > 0 && view.depth_shade > 0. {
            ui.painter().rect_filled(
                card,
                radius,
                tokens.color("glass-strong-solid").gamma_multiply(
                    captures_app::preview::collapsed_dim_opacity(view.depth) as f32
                        * view.depth_shade,
                ),
            );
        }
        ui.painter().rect_stroke(
            card,
            radius,
            Stroke::new(1., tokens.color("glass-border")),
            egui::StrokeKind::Inside,
        );
        paint_capture_highlight(ui, tokens, card, radius, view.highlight);
        if view.interactive {
            let response = ui.interact(
                card,
                ui.scope_id()
                    .with(("expand-preview-stack", view.artifact_id)),
                egui::Sense::click_and_drag(),
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    true,
                    format!("Expand {} previews", view.stack_count),
                )
            });
            if response.clicked() {
                action = Some(Action::ExpandStack);
            }
            let drag_id = response.id.with("desktop-drag");
            // winit can retain old local coordinates after moving the window.
            // Keep the original grab offset and use an OS desktop pointer sample.
            if response.is_pointer_button_down_on() {
                if ui.input(|input| input.pointer.primary_pressed()) {
                    ui.data_mut(|data| data.remove::<(egui::Pos2, Option<egui::Pos2>)>(drag_id));
                }
                let press = ui.input(|input| input.pointer.press_origin());
                if let Some(press) = press {
                    ui.data_mut(|data| {
                        data.get_temp_mut_or_insert_with(drag_id, || (press, None::<egui::Pos2>));
                    });
                }
            }
            if response.dragged_by(egui::PointerButton::Primary)
                && let Some(pointer) = view.desktop_pointer
                && let Some((press, previous)) =
                    ui.data(|data| data.get_temp::<(egui::Pos2, Option<egui::Pos2>)>(drag_id))
            {
                let position = pointer - press.to_vec2();
                if previous != Some(position) {
                    ui.data_mut(|data| data.insert_temp(drag_id, (press, Some(position))));
                    action = Some(Action::MoveStack(position));
                }
            }
            if !ui.input(|input| input.pointer.primary_down()) {
                ui.data_mut(|data| data.remove::<(egui::Pos2, Option<egui::Pos2>)>(drag_id));
            }
            // Shipping's collapsed hit target has no tooltip.
            response.on_hover_cursor(egui::CursorIcon::Grab);
        }
        return action;
    }

    let inset = 8.;
    let icon_size = egui::vec2(28., 28.);
    let gap = tokens.number("s-3");
    let tooltip_above = preview_chrome::icon_tooltip_above(view.top_anchor);
    let outer_x = if view.right_anchor {
        card.right() - inset - icon_size.x
    } else {
        card.left() + inset
    };
    let top_y = card.top() + inset;
    let copy_rect = egui::Rect::from_center_size(
        card.center() - egui::vec2(0., 16. + gap / 2.),
        egui::vec2(140., 32.),
    );
    // With Copy hidden, the remaining centered action sits at the card center.
    let save_rect = egui::Rect::from_center_size(
        card.center()
            + if view.clipboard_current {
                egui::Vec2::ZERO
            } else {
                egui::vec2(0., 16. + gap / 2.)
            },
        egui::vec2(140., 32.),
    );
    let editor_id = ui.scope_id().with(("edit", view.artifact_id));
    let edit_rect = editor_control_rect(ui, tokens, card, inset, &view, editor_id);
    let destructive_count: usize = if view.saved { 2 } else { 1 };
    let destructive_width =
        icon_size.x * destructive_count as f32 + gap * (destructive_count.saturating_sub(1)) as f32;
    let destructive_start = if view.right_anchor {
        outer_x + icon_size.x - destructive_width
    } else {
        outer_x
    };
    let close_rect = egui::Rect::from_min_size(egui::pos2(destructive_start, top_y), icon_size);
    let delete_rect = egui::Rect::from_min_size(
        egui::pos2(
            destructive_start + if view.saved { icon_size.x + gap } else { 0. },
            top_y,
        ),
        icon_size,
    );
    let pointer_over_control = ui
        .input(|input| {
            if input.pointer.primary_down() {
                input.pointer.press_origin()
            } else {
                input.pointer.hover_pos()
            }
        })
        .is_some_and(|pointer| {
            [save_rect, edit_rect, delete_rect]
                .iter()
                .any(|rect| rect.contains(pointer))
                || (!view.clipboard_current && copy_rect.contains(pointer))
                || (view.saved && close_rect.contains(pointer))
        });

    let card_hovered = ui
        .input(|input| input.pointer.hover_pos())
        .is_some_and(|pointer| card.contains(pointer));
    let any_focused = ui.memory(|memory| {
        ["close", "delete", "edit", "copy", "save-reveal"]
            .iter()
            .any(|name| memory.has_focus(ui.scope_id().with((name, view.artifact_id))))
    });
    // Shipping hover chrome: pointer hover (unless the stale-pointer lock
    // holds it off after an expand or a new capture) or keyboard focus.
    let reveal = view.interactive && ((card_hovered && !view.hover_locked) || any_focused);

    let ring = editor_ring_opacity(ui, tokens, &view);
    if ring > 0. {
        // `0 0 14px rgba(accent, .28)`: a glow outside the card, under it.
        ui.painter().add(
            egui::Shadow {
                offset: [0, 0],
                blur: 14,
                spread: 0,
                color: tokens.color("theme-accent").gamma_multiply(0.28 * ring),
            }
            .as_shape(card, radius),
        );
    }
    ui.painter()
        .rect_filled(card, radius, tokens.color("glass-raised"));
    let media = media_hover_progress(ui, tokens, &view, reveal);
    paint_media(
        ui,
        card,
        radius,
        view.texture,
        view.blurred,
        media.0,
        media.1,
    );
    ui.painter().rect_stroke(
        card,
        radius,
        Stroke::new(1., tokens.color("glass-border")),
        egui::StrokeKind::Inside,
    );
    paint_capture_highlight(ui, tokens, card, radius, view.highlight);
    if ring > 0. {
        // `0 0 0 2px rgba(accent, .9)`: the solid ring just outside the edge.
        ui.painter().rect_stroke(
            card,
            radius,
            Stroke::new(2., tokens.color("theme-accent").gamma_multiply(0.9 * ring)),
            egui::StrokeKind::Outside,
        );
    }

    if view.interactive && view.busy.is_none() {
        // Register the drag area every frame. egui hit-tests a press against
        // the previous frame's widgets, so skipping it while the pointer was
        // over a control would lose the next drag that starts off-control.
        // Controls are added later and stay on top for clicks.
        // Not focusable: shipping's draggable image is no Tab stop, so Tab
        // goes straight to the card controls (`:focus-within`).
        let response = ui.interact(
            card,
            ui.scope_id().with(("file-drag", view.artifact_id)),
            egui::Sense::DRAG,
        );
        if response.drag_started_by(egui::PointerButton::Primary) && !pointer_over_control {
            action = Some(Action::DragFile);
        }
        if !pointer_over_control {
            // Shipping shows the grab cursor on the image, with no tooltip.
            response.on_hover_cursor(egui::CursorIcon::Grab);
        }
    }
    let enabled = view.interactive && view.busy.is_none();
    let mut controls = Vec::new();
    if view.saved {
        controls.push((
            close_rect,
            "close",
            "Close",
            Action::Dismiss,
            Icon::Close,
            enabled,
        ));
    }
    controls.push((
        delete_rect,
        "delete",
        "Delete",
        if view.saved {
            Action::Trash
        } else {
            Action::Discard
        },
        Icon::Trash,
        enabled,
    ));

    for (rect, id, label, result, icon, control_enabled) in controls {
        if control(
            ui,
            tokens,
            rect,
            (id, view.artifact_id),
            label,
            icon,
            reveal,
            control_enabled,
            false,
            Some((tooltip_above, view.reduced_motion)),
        ) {
            action = Some(result);
        }
    }
    if editor_control(
        ui,
        tokens,
        edit_rect,
        editor_id,
        &view,
        reveal,
        enabled && view.can_save,
        tooltip_above,
    ) {
        action = Some(Action::Edit);
    }
    if !view.clipboard_current
        && control(
            ui,
            tokens,
            copy_rect,
            ("copy", view.artifact_id),
            "Copy",
            Icon::Copy,
            reveal,
            enabled,
            false,
            None,
        )
    {
        action = Some(Action::Copy);
    }
    let (second_label, second_icon) = if view.saved_feedback {
        ("Saved", Icon::Check)
    } else if view.saved {
        ("Show in Folder", Icon::Folder)
    } else {
        ("Save file", Icon::Save)
    };
    if control(
        ui,
        tokens,
        save_rect,
        ("save-reveal", view.artifact_id),
        second_label,
        second_icon,
        reveal,
        enabled && (view.saved || view.can_save),
        true,
        None,
    ) {
        action = Some(if view.saved {
            Action::Reveal
        } else {
            Action::Save
        });
    }

    let chip_pose = clipboard_chip_pose(ui, tokens, &view);
    if view.clipboard_current {
        let chip = clipboard_chip_rect(ui, tokens, card, inset);
        crate::motion::with_pose(ui, chip_pose, chip, |ui| {
            clipboard_chip(ui, tokens, chip);
        });
    }
    let label = view.message.map(str::to_owned).unwrap_or_else(|| {
        captures_app::preview::card_metadata(view.width, view.height, view.size_bytes)
    });
    let label_position = card.left_bottom() + egui::vec2(inset, -inset);
    let mut label_job = egui::text::LayoutJob::simple(
        label,
        egui::FontId::proportional(tokens.number("text-2xs")),
        tokens.color("glass-text"),
        card.width() - tokens.number("s-3") * 4.,
    );
    label_job.wrap.max_rows = 3;
    let galley = ui.painter().layout_job(label_job);
    let label_rect = egui::Rect::from_min_size(
        label_position - egui::vec2(0., galley.size().y + tokens.number("s-2") * 2.),
        galley.size() + egui::vec2(tokens.number("s-3") * 2., tokens.number("s-2") * 2.),
    );
    if view.message.is_some() || !reveal {
        ui.painter().rect_filled(
            label_rect,
            tokens.number("r-md"),
            tokens.color("glass-strong"),
        );
        ui.painter().galley(
            label_rect.min + egui::vec2(tokens.number("s-3"), tokens.number("s-2")),
            galley,
            tokens.color("glass-text"),
        );
        // `.thumbnail-meta .warning`: a second chip in the accent text colour.
        if let (None, Some(warning)) = (view.message, view.warning) {
            let text = ui.painter().layout_no_wrap(
                warning.to_owned(),
                egui::FontId::proportional(tokens.number("text-2xs")),
                tokens.color("theme-accent-text"),
            );
            let chip = egui::Rect::from_min_size(
                egui::pos2(
                    label_rect.right() + tokens.number("s-3"),
                    label_rect.bottom() - text.size().y - tokens.number("s-2") * 2.,
                ),
                text.size() + egui::vec2(tokens.number("s-3") * 2., tokens.number("s-2") * 2.),
            );
            ui.painter()
                .rect_filled(chip, tokens.number("r-md"), tokens.color("glass-strong"));
            ui.painter().galley(
                chip.min + egui::vec2(tokens.number("s-3"), tokens.number("s-2")),
                text,
                tokens.color("theme-accent-text"),
            );
        }
    }

    action
}

/// `thumbnail-capture-highlight`: `0 0 0 1px rgba(accent, .85), 0 0 18px
/// rgba(accent, .24)` fading out after a card arrives.
fn paint_capture_highlight(
    ui: &egui::Ui,
    tokens: &Tokens,
    card: egui::Rect,
    radius: f32,
    opacity: f32,
) {
    if opacity <= 0. {
        return;
    }
    let accent = tokens.color("theme-accent");
    ui.painter().add(
        egui::Shadow {
            offset: [0, 0],
            blur: 18,
            spread: 0,
            color: accent.gamma_multiply(0.24 * opacity),
        }
        .as_shape(card, radius),
    );
    ui.painter().rect_stroke(
        card,
        radius,
        Stroke::new(1., accent.gamma_multiply(0.85 * opacity)),
        egui::StrokeKind::Inside,
    );
}

/// Shipping `clipboard-confirmation-arrive`, timed from when this card's chip
/// appeared. Requests frames only while it plays.
fn clipboard_chip_pose(
    ui: &egui::Ui,
    tokens: &Tokens,
    view: &View<'_>,
) -> captures_app::motion::Pose {
    let id = ui.scope_id().with(("clipboard-chip", view.artifact_id));
    let now = ui.input(|input| input.time);
    let previous: Option<(bool, f64)> = ui.data(|data| data.get_temp(id));
    let since = match previous {
        Some((true, since)) if view.clipboard_current => since,
        _ if view.clipboard_current => now,
        _ => f64::NEG_INFINITY,
    };
    ui.data_mut(|data| data.insert_temp(id, (view.clipboard_current, since)));
    let arrive = tokens.motion(captures_app::motion::Motion::PreviewClipboardChipArrive);
    let elapsed = (now - since) * 1000.;
    if arrive.running(elapsed, view.reduced_motion) {
        ui.ctx().request_repaint();
    }
    arrive.pose_at(elapsed, view.reduced_motion)
}

/// A card playing its exit: painted in its held slot without chrome or input.
pub struct ExitView<'a> {
    pub texture: &'a egui::TextureHandle,
    pub blurred: Option<&'a egui::TextureHandle>,
    pub kind: captures_app::preview_motion::ExitKind,
    /// Milliseconds into the exit's own animation (after any Clear all delay).
    pub elapsed_ms: f64,
    pub dust: &'a [captures_app::preview_motion::DustParticle],
    pub right_anchor: bool,
    pub reduced_motion: bool,
}

/// Paint an exiting card in `card`. Shipping locks the hover look (blur and
/// half brightness) for every exit. Returns whether it is still moving.
pub fn show_exit(ui: &mut egui::Ui, tokens: &Tokens, card: egui::Rect, view: ExitView<'_>) -> bool {
    use captures_app::motion::Motion;
    use captures_app::preview_motion::ExitKind;
    let radius = tokens.number("thumbnail-card-radius");
    let elapsed = view.elapsed_ms.max(0.);
    let running = !view.reduced_motion && elapsed < view.kind.hold_ms();
    match view.kind {
        ExitKind::Dismiss | ExitKind::DeleteFallback => {
            let motion = if view.kind == ExitKind::Dismiss {
                Motion::PreviewDismiss
            } else {
                Motion::PreviewDeleteFallback
            };
            let mut pose = tokens.motion(motion).pose_at(elapsed, view.reduced_motion);
            if view.right_anchor {
                pose.translate_x = -pose.translate_x;
            }
            // The streak stretches and blurs the media inside its clip.
            let streak = if view.kind == ExitKind::Dismiss {
                tokens
                    .motion(Motion::PreviewDismissStreak)
                    .pose_at(elapsed, view.reduced_motion)
            } else {
                captures_app::motion::Pose { scale: 1., ..pose }
            };
            crate::motion::with_pose(ui, pose, card, |ui| {
                ui.painter()
                    .rect_filled(card, radius, tokens.color("glass-raised"));
                paint_streaked_media(ui, card, radius, &view, streak);
                ui.painter().rect_stroke(
                    card,
                    radius,
                    Stroke::new(1., tokens.color("glass-border")),
                    egui::StrokeKind::Inside,
                );
            });
        }
        ExitKind::Dust => paint_dust(ui, tokens, card, radius, &view, elapsed),
    }
    running
}

/// The locked hover media, stretched by `streak.scale_x` and smeared into a
/// horizontal motion blur of `streak.blur` points: equal-weight copies, each
/// blended at 1/k so the result is their average.
fn paint_streaked_media(
    ui: &egui::Ui,
    card: egui::Rect,
    radius: f32,
    view: &ExitView<'_>,
    streak: captures_app::motion::Pose,
) {
    use captures_app::preview_chrome::HOVER_MEDIA_BRIGHTNESS;
    let (scale_x, scale_y) = streak.scales();
    let rect = egui::Rect::from_center_size(
        card.center(),
        egui::vec2(
            card.width() * scale_x as f32,
            card.height() * scale_y as f32,
        ),
    );
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(card));
    let (texture, uv) = match view.blurred {
        Some(blurred) => (
            blurred.id(),
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
        ),
        None => (
            view.texture.id(),
            cover_uv(view.texture.size_vec2(), card.size()),
        ),
    };
    let copies = if streak.blur > 2.5 { 7 } else { 1 };
    let spread = streak.blur as f32;
    for copy in 0..copies {
        let offset = if copies == 1 {
            0.
        } else {
            spread * (copy as f32 / (copies - 1) as f32 * 2. - 1.)
        };
        let alpha = 1. / (copy + 1) as f32;
        let value = (HOVER_MEDIA_BRIGHTNESS as f32 * alpha * 255.).round() as u8;
        painter.add(
            egui::epaint::RectShape::filled(
                rect.translate(egui::vec2(offset, 0.)),
                radius,
                Color32::from_rgba_premultiplied(value, value, value, (alpha * 255.).round() as u8),
            )
            .with_texture(texture, uv),
        );
    }
}

/// Shipping dust delete: the frozen hover image fades under a mesh of image
/// chips that rise and scatter from the trash control while the clip opens.
fn paint_dust(
    ui: &egui::Ui,
    tokens: &Tokens,
    card: egui::Rect,
    radius: f32,
    view: &ExitView<'_>,
    elapsed: f64,
) {
    use captures_app::preview_chrome::HOVER_MEDIA_BRIGHTNESS;
    use captures_app::preview_motion::{
        DUST_LAYER_PAD, delete_frame_opacity, dust_frame, dust_visual_at,
    };
    let frame = dust_frame(elapsed, view.reduced_motion);
    let border = delete_frame_opacity(tokens, elapsed, view.reduced_motion) as f32;
    let (texture, base) = match view.blurred {
        Some(blurred) => (
            blurred.id(),
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
        ),
        None => (
            view.texture.id(),
            cover_uv(view.texture.size_vec2(), card.size()),
        ),
    };
    let tint = |alpha: f32| {
        let value = (HOVER_MEDIA_BRIGHTNESS as f32 * alpha * 255.).round() as u8;
        Color32::from_rgba_premultiplied(value, value, value, (alpha * 255.).round() as u8)
    };
    if border > 0. {
        ui.painter().add(
            egui::Shadow {
                offset: [0, 6],
                blur: 14,
                spread: 0,
                color: Color32::from_black_alpha((0.38 * 255. * border) as u8),
            }
            .as_shape(card, radius),
        );
    }
    if frame.source_opacity > 0. {
        ui.painter()
            .with_clip_rect(ui.clip_rect().intersect(card))
            .add(
                egui::epaint::RectShape::filled(card, radius, tint(frame.source_opacity as f32))
                    .with_texture(texture, base),
            );
    }
    if frame.layer_opacity > 0. {
        let pad = DUST_LAYER_PAD as f32;
        let layer = card.min - egui::vec2(pad, pad);
        let clip = card.expand(pad * frame.clip_open as f32);
        let mut mesh = egui::Mesh::with_texture(texture);
        let size = card.size();
        for particle in view.dust {
            let visual = dust_visual_at(particle, elapsed);
            let alpha = (visual.opacity * frame.layer_opacity) as f32;
            if alpha <= 0. {
                continue;
            }
            let half = egui::vec2(particle.width as f32, particle.height as f32) / 2.;
            let centre = layer
                + egui::vec2(particle.left as f32, particle.top as f32)
                + half
                + egui::vec2(visual.dx as f32, visual.dy as f32);
            let (sin, cos) = (visual.rotate.to_radians() as f32).sin_cos();
            let scale = visual.scale as f32;
            let uv_min = base.min
                + egui::vec2(
                    particle.source_left as f32 / size.x * base.width(),
                    particle.source_top as f32 / size.y * base.height(),
                );
            let uv_size = egui::vec2(
                particle.width as f32 / size.x * base.width(),
                particle.height as f32 / size.y * base.height(),
            );
            let color = tint(alpha);
            let first = mesh.vertices.len() as u32;
            for (corner, uv) in [
                (egui::vec2(-1., -1.), egui::vec2(0., 0.)),
                (egui::vec2(1., -1.), egui::vec2(1., 0.)),
                (egui::vec2(1., 1.), egui::vec2(1., 1.)),
                (egui::vec2(-1., 1.), egui::vec2(0., 1.)),
            ] {
                let local = egui::vec2(corner.x * half.x, corner.y * half.y) * scale;
                let rotated =
                    egui::vec2(local.x * cos - local.y * sin, local.x * sin + local.y * cos);
                mesh.vertices.push(egui::epaint::Vertex {
                    pos: centre + rotated,
                    uv: uv_min + egui::vec2(uv.x * uv_size.x, uv.y * uv_size.y),
                    color,
                });
            }
            mesh.indices.extend_from_slice(&[
                first,
                first + 1,
                first + 2,
                first,
                first + 2,
                first + 3,
            ]);
        }
        ui.painter()
            .with_clip_rect(ui.clip_rect().intersect(clip))
            .add(egui::Shape::mesh(mesh));
    }
    if border > 0. {
        ui.painter().rect_stroke(
            card,
            radius,
            Stroke::new(1., Color32::from_white_alpha((0.08 * 255. * border) as u8)),
            egui::StrokeKind::Inside,
        );
    }
}

/// Shipping `thumbnail-stack-sparkle`: two layers of accent and white dots
/// drifting up over a hovered collapsed pile. `elapsed_ms` counts from the
/// hover's start; the layers loop until the hover ends.
pub fn paint_sparkles(
    ui: &egui::Ui,
    tokens: &Tokens,
    target: egui::Rect,
    top_anchor: bool,
    elapsed_ms: f64,
    reduced_motion: bool,
) -> bool {
    use captures_app::motion::Motion;
    use captures_app::preview_motion::{
        SPARKLES_EARLY, SPARKLES_LATE, sparkle_center, sparkle_layer,
    };
    if reduced_motion {
        return false;
    }
    let (x, y, width, height) = sparkle_layer(
        (
            f64::from(target.left()),
            f64::from(target.top()),
            f64::from(target.width()),
            f64::from(target.height()),
        ),
        top_anchor,
    );
    let centre = egui::pos2((x + width / 2.) as f32, (y + height / 2.) as f32);
    let accent = tokens.color("theme-accent");
    for (motion, dots) in [
        (Motion::PreviewPileSparkle, &SPARKLES_EARLY[..]),
        (Motion::PreviewPileSparkleLate, &SPARKLES_LATE[..]),
    ] {
        let pose = tokens.motion(motion).pose_repeating(elapsed_ms, false);
        if pose.opacity <= 0. {
            continue;
        }
        let scale = pose.scale as f32;
        for dot in dots {
            let (dx, dy) = sparkle_center(dot, (x, y, width, height));
            let point = centre
                + (egui::pos2(dx as f32, dy as f32) - centre) * scale
                + egui::vec2(0., pose.translate_y as f32);
            let base = if dot.accent { accent } else { Color32::WHITE };
            let alpha = (dot.alpha * pose.opacity) as f32;
            // The radial gradient's soft edge, then its solid core.
            ui.painter().circle_filled(
                point,
                dot.fade as f32 * scale,
                base.gamma_multiply(alpha * 0.35),
            );
            ui.painter()
                .circle_filled(point, dot.core as f32 * scale, base.gamma_multiply(alpha));
        }
    }
    true
}

/// Eased 0…1 progress of the hover filter (blur and brightness) and scale,
/// each on its shipping transition. egui animates only while they change.
fn media_hover_progress(
    ui: &egui::Ui,
    tokens: &Tokens,
    view: &View<'_>,
    reveal: bool,
) -> (f32, f32) {
    use captures_app::motion::Transition;
    let progress = |transition: Transition, name: &str| {
        let tween = tokens.transition(transition);
        let seconds = if view.reduced_motion {
            0.
        } else {
            tween.duration_ms as f32 / 1000.
        };
        let linear = ui.ctx().animate_bool_with_time(
            ui.scope_id().with((name, view.artifact_id)),
            reveal,
            seconds,
        );
        tween.easing.ease(f64::from(linear)) as f32
    };
    (
        progress(Transition::PreviewMediaFilter, "media-filter"),
        progress(Transition::PreviewMediaScale, "media-scale"),
    )
}

/// Shipping `.thumbnail-media img`: cover-cropped media, clipped to the card.
/// On hover it takes `blur(2px) brightness(.5) scale(1.015)`. egui has no
/// per-image blur, so a pre-blurred card-sized copy fades in over the sharp
/// image while both darken and scale.
fn paint_media(
    ui: &egui::Ui,
    card: egui::Rect,
    radius: f32,
    texture: &egui::TextureHandle,
    blurred: Option<&egui::TextureHandle>,
    filter: f32,
    scale: f32,
) {
    use captures_app::preview_chrome::{HOVER_MEDIA_BRIGHTNESS, HOVER_MEDIA_SCALE};
    let scale = egui::lerp(1.0..=HOVER_MEDIA_SCALE as f32, scale);
    let rect = egui::Rect::from_center_size(card.center(), card.size() * scale);
    let brightness = egui::lerp(1.0..=HOVER_MEDIA_BRIGHTNESS as f32, filter);
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(card));
    let tint = |alpha: f32| {
        let value = (brightness * alpha * 255.).round() as u8;
        Color32::from_rgba_premultiplied(value, value, value, (alpha * 255.).round() as u8)
    };
    let corners = radius * scale;
    painter.add(
        egui::epaint::RectShape::filled(rect, corners, tint(1.))
            .with_texture(texture.id(), cover_uv(texture.size_vec2(), card.size())),
    );
    if let Some(blurred) = blurred.filter(|_| filter > 0.) {
        painter.add(
            egui::epaint::RectShape::filled(rect, corners, tint(filter)).with_texture(
                blurred.id(),
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
            ),
        );
    }
}

/// Opacity of the editor ring: it arrives over `0.22s ease` and eases out
/// over the 550 ms leave.
fn editor_ring_opacity(ui: &egui::Ui, tokens: &Tokens, view: &View<'_>) -> f32 {
    use captures_app::motion::Transition;
    let (transition, from, to) = match view.editor {
        EditorPhase::Present => (Transition::PreviewEditorRing, 0., 1.),
        EditorPhase::Leaving => (Transition::PreviewEditorRingLeave, 1., 0.),
        EditorPhase::Idle | EditorPhase::Lingering => return 0.,
    };
    let tween = tokens.transition(transition);
    if tween.running(view.editor_elapsed_ms, view.reduced_motion) {
        ui.ctx().request_repaint();
    }
    tween.value(from, to, view.editor_elapsed_ms, view.reduced_motion) as f32
}

/// Width of the present pill for these tokens: both labels share one cell.
fn editor_pill_width(ui: &egui::Ui, tokens: &Tokens) -> f32 {
    let measure = |text: &str| {
        ui.painter()
            .layout_no_wrap(
                text.to_owned(),
                egui::FontId::proportional(tokens.number("text-2xs")),
                Color32::WHITE,
            )
            .size()
            .x
    };
    preview_chrome::editor_pill_width(
        f64::from(measure(preview_chrome::EDITOR_PRESENT_LABEL)),
        f64::from(measure(preview_chrome::EDITOR_SHOW_LABEL)),
    ) as f32
}

/// The editor control's rect: a 28 pt icon in the inner top corner that
/// widens away from that corner into the present pill over the shipping morph.
fn editor_control_rect(
    ui: &egui::Ui,
    tokens: &Tokens,
    card: egui::Rect,
    inset: f32,
    view: &View<'_>,
    id: egui::Id,
) -> egui::Rect {
    let size = preview_chrome::EDITOR_CONTROL_SIZE as f32;
    let target = if view.editor.present() {
        editor_pill_width(ui, tokens)
    } else {
        size
    };
    let morph = tokens.transition(captures_app::motion::Transition::PreviewEditorMorph);
    let seconds = if view.reduced_motion {
        0.
    } else {
        morph.duration_ms as f32 / 1000.
    };
    let width = ui
        .ctx()
        .animate_value_with_time(id.with("width"), target, seconds);
    let top = card.top() + inset;
    if view.right_anchor {
        egui::Rect::from_min_size(
            egui::pos2(card.left() + inset, top),
            egui::vec2(width, size),
        )
    } else {
        egui::Rect::from_min_max(
            egui::pos2(card.right() - inset - width, top),
            egui::pos2(card.right() - inset, top + size),
        )
    }
}

/// Shipping `.thumbnail-editor-control`: the compact Edit icon, or while an
/// editor shows this capture the "In editor" pill that offers "Show in
/// editor" on hover or focus. Returns whether it was activated.
#[allow(clippy::too_many_arguments)]
fn editor_control(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    rect: egui::Rect,
    id: egui::Id,
    view: &View<'_>,
    reveal: bool,
    enabled: bool,
    tooltip_above: bool,
) -> bool {
    let phase = view.editor;
    let response = ui.interact(rect, id, egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, phase.accessible_label())
    });
    // `data-editor-just-opened`: the click that opened the editor keeps the
    // pill passive until the pointer leaves the control.
    let latch = id.with("just-opened");
    let hovered = response.hovered();
    let mut just_opened = ui.data(|data| data.get_temp::<bool>(latch)) == Some(true);
    if just_opened && !hovered {
        ui.data_mut(|data| data.remove::<bool>(latch));
        just_opened = false;
    }
    let focused = response.has_focus();
    let visible = reveal || focused || phase.pinned();
    if visible {
        let action_hover = (hovered || focused) && enabled && phase.interactive();
        if phase.present() {
            let offers_action = action_hover && !just_opened;
            let accent = tokens.color("theme-accent");
            ui.painter().add(
                egui::Shadow {
                    offset: [0, 0],
                    blur: 14,
                    spread: 0,
                    color: accent.gamma_multiply(0.2),
                }
                .as_shape(rect, rect.height() / 2.),
            );
            let (fill, border, color) = if offers_action {
                (
                    accent,
                    Color32::TRANSPARENT,
                    tokens.color("theme-accent-ink"),
                )
            } else {
                (
                    tokens.color("glass-strong"),
                    accent.gamma_multiply(0.72),
                    tokens.color("theme-accent-text"),
                )
            };
            ui.painter().rect(
                rect,
                rect.height() / 2.,
                fill,
                Stroke::new(1., border),
                egui::StrokeKind::Inside,
            );
            let icon_side = preview_chrome::EDITOR_PILL_ICON as f32;
            let icon = egui::Rect::from_min_size(
                egui::pos2(
                    rect.left() + 1. + preview_chrome::EDITOR_PILL_PADDING_LEFT as f32,
                    rect.center().y - icon_side / 2.,
                ),
                egui::vec2(icon_side, icon_side),
            );
            // `stroke-width: 2.2` in the 24-unit viewBox at 11 pt.
            paint_icon_with(ui.painter(), Icon::Edit, icon, color, 2.2 * icon_side / 24.);
            let label = ui.painter().layout_no_wrap(
                preview_chrome::editor_pill_label(hovered || focused, just_opened).to_owned(),
                egui::FontId::proportional(tokens.number("text-2xs")),
                color,
            );
            let text = egui::pos2(
                icon.right() + preview_chrome::EDITOR_PILL_GAP as f32,
                rect.center().y - label.size().y / 2.,
            );
            // Clip the label while the pill is still widening.
            ui.painter()
                .with_clip_rect(rect.shrink(1.))
                .galley(text, label, color);
        } else {
            ui.painter().rect(
                rect,
                tokens.number("r-md"),
                tokens.color(if action_hover {
                    "glass-raised"
                } else {
                    "glass-strong"
                }),
                Stroke::new(1., tokens.color("glass-border")),
                egui::StrokeKind::Inside,
            );
            let icon = egui::Rect::from_center_size(rect.center(), egui::vec2(16., 16.));
            paint_icon(
                ui.painter(),
                Icon::Edit,
                icon,
                tokens.color(if enabled {
                    "glass-text"
                } else {
                    "glass-text-muted"
                }),
            );
        }
        if focused {
            ui.painter().rect_stroke(
                rect.expand(2.),
                if phase.present() {
                    rect.height() / 2. + 2.
                } else {
                    tokens.number("r-md")
                },
                Stroke::new(2., tokens.color("theme-accent")),
                egui::StrokeKind::Outside,
            );
        }
    }
    let tip = tooltip_progress(
        ui,
        tokens,
        id,
        visible && phase.tooltip().is_some() && (hovered || focused),
        captures_app::motion::Transition::PreviewIconTooltip,
        view.reduced_motion,
    );
    if let Some(text) = phase.tooltip() {
        crate::glass_tooltip::preview_icon(ui, tokens, rect, text, tooltip_above, tip);
    }
    let activated = visible && enabled && phase.interactive() && response.clicked();
    if activated {
        ui.data_mut(|data| data.insert_temp(latch, true));
    }
    activated
}

/// 0…1 progress of an instant tooltip: no delay, fading and nudging over
/// its shipping transition.
fn tooltip_progress(
    ui: &egui::Ui,
    tokens: &Tokens,
    id: egui::Id,
    showing: bool,
    transition: captures_app::motion::Transition,
    reduced_motion: bool,
) -> f32 {
    let tween = tokens.transition(transition);
    let seconds = if reduced_motion {
        0.
    } else {
        tween.duration_ms as f32 / 1000.
    };
    let linear = ui
        .ctx()
        .animate_bool_with_time(id.with("tooltip"), showing, seconds);
    tween.easing.ease(f64::from(linear)) as f32
}

/// Where the clipboard chip sits: the card's bottom-right corner.
fn clipboard_chip_rect(ui: &egui::Ui, tokens: &Tokens, card: egui::Rect, inset: f32) -> egui::Rect {
    let text = ui.painter().layout_no_wrap(
        "Copied to clipboard".to_owned(),
        egui::FontId::proportional(tokens.number("text-2xs")),
        Color32::from_rgb(0xea, 0xff, 0xf0),
    );
    let size = egui::vec2(
        tokens.number("s-4") * 2. + 12. + tokens.number("s-2") + text.size().x,
        3. * 2. + text.size().y.max(12.),
    );
    egui::Rect::from_min_size(card.right_bottom() - egui::vec2(inset, inset) - size, size)
}

/// Shipping `.clipboard-confirmation`: a green-edged pill at the bottom right.
fn clipboard_chip(ui: &egui::Ui, tokens: &Tokens, rect: egui::Rect) {
    let text = ui.painter().layout_no_wrap(
        "Copied to clipboard".to_owned(),
        egui::FontId::proportional(tokens.number("text-2xs")),
        Color32::from_rgb(0xea, 0xff, 0xf0),
    );
    let padding = egui::vec2(tokens.number("s-4"), 3.);
    let icon = 12.;
    let gap = tokens.number("s-2");
    let size = rect.size();
    ui.painter().rect(
        rect,
        size.y / 2.,
        Color32::from_rgba_unmultiplied(10, 22, 15, 230),
        Stroke::new(1., Color32::from_rgba_unmultiplied(53, 163, 93, 140)),
        egui::StrokeKind::Inside,
    );
    let icon_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left() + padding.x, rect.center().y - icon / 2.),
        egui::vec2(icon, icon),
    );
    paint_icon(
        ui.painter(),
        Icon::Check,
        icon_rect,
        Color32::from_rgb(0x7f, 0xd7, 0x9c),
    );
    ui.painter().galley(
        egui::pos2(
            icon_rect.right() + gap,
            rect.center().y - text.size().y / 2.,
        ),
        text,
        Color32::from_rgb(0xea, 0xff, 0xf0),
    );
}

pub fn show_stack_controls(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    right_anchor: bool,
    top_anchor: bool,
    reduced_motion: bool,
) -> Option<StackAction> {
    use captures_app::preview::{
        STACK_CLEAR_ALL_LABEL, STACK_CLEAR_ALL_TOOLTIP, STACK_MINIMIZE_HOVER_LABEL,
        STACK_MINIMIZE_HOVER_WIDTH, STACK_MINIMIZE_LABEL,
    };
    let mut action = None;
    let size = egui::vec2(28., 28.);
    let gap = tokens.number("s-1");
    let outer_x = if right_anchor {
        ui.max_rect().right() - 28.
    } else {
        ui.max_rect().left()
    };
    let inner_x = outer_x
        + if right_anchor {
            -(28. + gap)
        } else {
            28. + gap
        };
    let y = ui.max_rect().center().y - 14.;
    // Clear all stays at the outside edge so the Show less pill grows inward
    // and never covers it.
    let clear = egui::Rect::from_min_size(egui::pos2(outer_x, y), size);
    let clear_response = stack_button(ui, tokens, clear, "clear-all", STACK_CLEAR_ALL_LABEL, true);
    paint_icon(
        ui.painter(),
        Icon::Close,
        egui::Rect::from_center_size(clear.center(), egui::vec2(14., 14.)),
        tokens.color("glass-text"),
    );
    // Shipping `.thumbnail-stack-control[data-tooltip]`: an instant glass tip,
    // above the bottom toolbar and below the top one.
    let tip = tooltip_progress(
        ui,
        tokens,
        clear_response.id,
        clear_response.hovered() || clear_response.has_focus(),
        captures_app::motion::Transition::PreviewStackTooltip,
        reduced_motion,
    );
    crate::glass_tooltip::preview_icon(
        ui,
        tokens,
        clear,
        STACK_CLEAR_ALL_TOOLTIP,
        preview_chrome::icon_tooltip_above(top_anchor),
        tip,
    );
    if clear_response.clicked() {
        action = Some(StackAction::ClearAll);
    }

    // The pill keeps its hover width until the pointer leaves the wide
    // bounds, so growing under a stationary pointer cannot flicker.
    let minimize_id = ui.scope_id().with("show-less");
    let expanded = ui.data(|data| data.get_temp::<bool>(minimize_id.with("expanded")))
        == Some(true)
        || ui.memory(|memory| memory.has_focus(minimize_id));
    let anchored = |width: f32| {
        if right_anchor {
            egui::Rect::from_min_max(
                egui::pos2(inner_x + 28. - width, y),
                egui::pos2(inner_x + 28., y + 28.),
            )
        } else {
            egui::Rect::from_min_size(egui::pos2(inner_x, y), egui::vec2(width, 28.))
        }
    };
    let minimize = anchored(if expanded {
        STACK_MINIMIZE_HOVER_WIDTH as f32
    } else {
        28.
    });
    // Shipping morph: the width eases over 240 ms while the icon gives way to
    // the label over 180 ms, both `--ease-out`.
    let morph = captures_app::preview_motion::MinimizeMorph {
        width: eased_toggle(
            ui,
            minimize_id.with("morph"),
            expanded,
            captures_app::motion::Transition::PreviewMinimizeMorph,
            tokens,
            reduced_motion,
        ),
        swap: eased_toggle(
            ui,
            minimize_id.with("swap"),
            expanded,
            captures_app::motion::Transition::PreviewMinimizeSwap,
            tokens,
            reduced_motion,
        ),
    };
    let painted = anchored(morph.width_between(28., STACK_MINIMIZE_HOVER_WIDTH) as f32);
    let response = stack_button_at(
        ui,
        tokens,
        minimize,
        painted,
        "show-less",
        STACK_MINIMIZE_LABEL,
        !expanded,
    );
    let hover = response.hovered() || response.has_focus();
    if hover != expanded {
        ui.data_mut(|data| data.insert_temp(minimize_id.with("expanded"), hover));
        ui.ctx().request_repaint();
    }
    // `--thumbnail-minimize-slide`: the icon leaves toward, and the label
    // arrives from, the pile's screen edge.
    let slide = if right_anchor { -1. } else { 1. };
    let clip = ui
        .painter()
        .with_clip_rect(painted.intersect(ui.clip_rect()));
    let (icon_opacity, icon_x, icon_scale) = morph.icon();
    if icon_opacity > 0. {
        let rest = anchored(28.);
        paint_icon(
            &clip,
            Icon::Stack,
            egui::Rect::from_center_size(
                rest.center() + egui::vec2(icon_x as f32 * slide, 0.),
                egui::vec2(14., 14.) * icon_scale as f32,
            ),
            tokens
                .color("glass-text")
                .gamma_multiply(icon_opacity as f32),
        );
    }
    let (label_opacity, label_x) = morph.label();
    if label_opacity > 0. {
        clip.text(
            painted.center() + egui::vec2(label_x as f32 * slide, 0.),
            egui::Align2::CENTER_CENTER,
            STACK_MINIMIZE_HOVER_LABEL,
            egui::FontId::proportional(tokens.number("text-2xs")),
            tokens
                .color("glass-text")
                .gamma_multiply(label_opacity as f32),
        );
    }
    if response.clicked() {
        action = Some(StackAction::ToggleCollapsed);
    }
    action
}

/// 0…1 progress of a toggled transition, eased in its direction of travel
/// like a CSS transition (reversing plays the curve backward from 1).
fn eased_toggle(
    ui: &egui::Ui,
    id: egui::Id,
    on: bool,
    transition: captures_app::motion::Transition,
    tokens: &Tokens,
    reduced_motion: bool,
) -> f64 {
    let tween = tokens.transition(transition);
    let seconds = if reduced_motion {
        0.
    } else {
        tween.duration_ms as f32 / 1000.
    };
    let linear = f64::from(ui.ctx().animate_bool_with_time(id, on, seconds));
    if on {
        tween.easing.ease(linear)
    } else {
        1. - tween.easing.ease(1. - linear)
    }
}

/// Shipping `.thumbnail-stack-control`: solid glass square, raised on hover
/// (the expanded Show less pill keeps the resting glass instead).
fn stack_button(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    rect: egui::Rect,
    id: &str,
    label: &str,
    raise_on_hover: bool,
) -> egui::Response {
    stack_button_at(ui, tokens, rect, rect, id, label, raise_on_hover)
}

/// [`stack_button`] hit-tested at `rect` and painted at `paint` (the Show
/// less pill paints its animated width but keeps the target's hit area).
fn stack_button_at(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    rect: egui::Rect,
    paint: egui::Rect,
    id: &str,
    label: &str,
    raise_on_hover: bool,
) -> egui::Response {
    let response = ui.interact(rect, ui.scope_id().with(id), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    let (fill, border) = if response.hovered() && raise_on_hover {
        ("glass-raised-solid", "glass-border-strong")
    } else {
        ("glass-strong-solid", "glass-border")
    };
    ui.painter().rect(
        paint,
        tokens.number("r-md"),
        tokens.color(fill),
        Stroke::new(1., tokens.color(border)),
        egui::StrokeKind::Inside,
    );
    if response.has_focus() {
        ui.painter().rect_stroke(
            paint.expand(2.),
            tokens.number("r-md"),
            Stroke::new(2., tokens.color("theme-accent")),
            egui::StrokeKind::Outside,
        );
    }
    response
}

/// Shipping overflow cues: centered chevron tabs at the window's top and
/// bottom edges while an expanded stack hides cards there. Returns the number
/// of card slots to scroll (negative is up).
pub fn show_overflow_cues(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    window: egui::Rect,
    overflow: captures_app::preview::StackOverflow,
    top_anchor: bool,
) -> Option<i32> {
    let mut slots = None;
    let size = egui::vec2(46., 22.);
    let radius = tokens.number("r-lg");
    for (above, visible) in [(true, overflow.above), (false, overflow.below)] {
        if !visible {
            continue;
        }
        let rect = egui::Rect::from_center_size(
            egui::pos2(
                window.center().x,
                if above {
                    window.top() + 6. + size.y / 2.
                } else {
                    window.bottom() - 6. - size.y / 2.
                },
            ),
            size,
        );
        let label = captures_app::preview::overflow_cue_label(above, top_anchor);
        let response = ui.interact(
            rect,
            ui.scope_id().with(("overflow-cue", above)),
            egui::Sense::click(),
        );
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
        let corners = if above {
            egui::CornerRadius {
                nw: 0,
                ne: 0,
                sw: radius as u8,
                se: radius as u8,
            }
        } else {
            egui::CornerRadius {
                nw: radius as u8,
                ne: radius as u8,
                sw: 0,
                se: 0,
            }
        };
        let paint_rect = if response.hovered() {
            egui::Rect::from_center_size(rect.center(), size * 1.05)
        } else {
            rect
        };
        ui.painter().rect(
            paint_rect,
            corners,
            tokens.color(if response.hovered() {
                "glass-raised"
            } else {
                "glass-strong"
            }),
            Stroke::new(1., tokens.color("glass-border")),
            egui::StrokeKind::Inside,
        );
        let chevron = egui::Rect::from_center_size(rect.center(), egui::vec2(16., 16.));
        let point = |x: f32, y: f32| {
            egui::pos2(
                chevron.left() + x / 16. * chevron.width(),
                chevron.top() + y / 16. * chevron.height(),
            )
        };
        let points = if above {
            [point(3.5, 10.), point(8., 5.5), point(12.5, 10.)]
        } else {
            [point(3.5, 6.), point(8., 10.5), point(12.5, 6.)]
        };
        ui.painter().add(egui::Shape::line(
            points.to_vec(),
            Stroke::new(2., tokens.color("glass-text")),
        ));
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect.expand(2.),
                corners,
                Stroke::new(2., tokens.color("theme-accent")),
                egui::StrokeKind::Outside,
            );
        }
        // Shipping cues are named for accessibility but carry no tooltip.
        if response.clicked() {
            slots = Some(if above { -1 } else { 1 });
        }
    }
    slots
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Icon {
    Close,
    Trash,
    Edit,
    Copy,
    Save,
    Folder,
    Stack,
    Check,
}

#[allow(clippy::too_many_arguments)]
fn control(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    rect: egui::Rect,
    id_source: impl std::hash::Hash + std::fmt::Debug,
    label: &str,
    icon: Icon,
    reveal: bool,
    enabled: bool,
    primary: bool,
    tooltip: Option<(bool, bool)>,
) -> bool {
    let id = ui.scope_id().with(id_source);
    let response = ui.interact(rect, id, egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    let visible = reveal || response.has_focus();
    if visible {
        let fill = if primary {
            tokens.color("theme-accent")
        } else if response.hovered() {
            tokens.color("glass-raised")
        } else {
            tokens.color("glass-strong")
        };
        ui.painter().rect(
            rect,
            tokens.number("r-md"),
            fill,
            Stroke::new(1., tokens.color("glass-border")),
            egui::StrokeKind::Inside,
        );
        let color = if primary {
            tokens.color("theme-accent-ink")
        } else if enabled {
            if matches!(icon, Icon::Trash) {
                tokens.color("theme-signal-text")
            } else {
                tokens.color("glass-text")
            }
        } else {
            tokens.color("glass-text-muted")
        };
        let label = (rect.width() > 40.).then(|| {
            ui.painter().layout_no_wrap(
                label.to_owned(),
                egui::FontId::proportional(tokens.number("text-xs")),
                color,
            )
        });
        let gap = tokens.number("s-3");
        let icon_center = label.as_ref().map_or(rect.center(), |text| {
            rect.center() - egui::vec2((text.size().x + gap) / 2., 0.)
        });
        // `.thumbnail-main-actions svg`: a new icon pops in (Save → Saved →
        // Show in Folder). Icon buttons keep a still glyph.
        let pop = if tooltip.is_none() {
            action_icon_pop(ui, tokens, id, icon)
        } else {
            captures_app::motion::Pose::REST
        };
        paint_icon(
            ui.painter(),
            icon,
            egui::Rect::from_center_size(icon_center, egui::vec2(16., 16.) * pop.scale as f32),
            color.gamma_multiply(pop.opacity as f32),
        );
        if let Some(text) = label {
            ui.painter().galley(
                egui::pos2(
                    icon_center.x + 8. + gap,
                    rect.center().y - text.size().y / 2.,
                ),
                text,
                color,
            );
        }
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect.expand(2.),
                tokens.number("r-md"),
                Stroke::new(2., tokens.color("theme-accent")),
                egui::StrokeKind::Outside,
            );
        }
    }
    // Shipping icon buttons carry an instant glass tip; the labelled centre
    // actions carry none. `tooltip` is (opens above, reduced motion).
    if let Some((above, reduced_motion)) = tooltip {
        let progress = tooltip_progress(
            ui,
            tokens,
            id,
            visible && (response.hovered() || response.has_focus()),
            captures_app::motion::Transition::PreviewIconTooltip,
            reduced_motion,
        );
        crate::glass_tooltip::preview_icon(ui, tokens, rect, label, above, progress);
    }
    visible && enabled && response.clicked()
}

/// Shipping `thumbnail-action-pop`, timed from when `icon` replaced the
/// control's previous glyph. The first glyph a control shows does not pop:
/// shipping mounts it while the actions are still hidden.
fn action_icon_pop(
    ui: &egui::Ui,
    tokens: &Tokens,
    id: egui::Id,
    icon: Icon,
) -> captures_app::motion::Pose {
    let key = id.with("icon-pop");
    let now = ui.input(|input| input.time);
    let since = match ui.data(|data| data.get_temp::<(Icon, f64)>(key)) {
        Some((previous, since)) if previous == icon => since,
        Some(_) => now,
        None => f64::NEG_INFINITY,
    };
    ui.data_mut(|data| data.insert_temp(key, (icon, since)));
    let reduced = crate::motion::reduced(ui.ctx());
    let pop = tokens.motion(captures_app::motion::Motion::PreviewActionIconPop);
    let elapsed = (now - since) * 1000.;
    if pop.running(elapsed, reduced) {
        ui.ctx().request_repaint();
    }
    pop.pose_at(elapsed, reduced)
}

fn paint_icon(p: &egui::Painter, icon: Icon, rect: egui::Rect, color: Color32) {
    paint_icon_with(p, icon, rect, color, 1.8);
}

/// Shipping 24-unit icons at `rect` with a `width` point stroke.
fn paint_icon_with(p: &egui::Painter, icon: Icon, rect: egui::Rect, color: Color32, width: f32) {
    let q = |x: f32, y: f32| {
        egui::pos2(
            rect.left() + x / 24. * rect.width(),
            rect.top() + y / 24. * rect.height(),
        )
    };
    let stroke = Stroke::new(width, color);
    let line = |points: &[(f32, f32)]| {
        p.add(egui::Shape::line(
            points.iter().map(|&(x, y)| q(x, y)).collect(),
            stroke,
        ))
    };
    match icon {
        Icon::Close => {
            line(&[(6., 6.), (18., 18.)]);
            line(&[(18., 6.), (6., 18.)]);
        }
        Icon::Check => {
            line(&[(5., 12.), (9., 16.), (19., 6.)]);
        }
        Icon::Edit => {
            line(&[
                (4., 16.),
                (3., 21.),
                (8., 20.),
                (19., 9.),
                (15., 5.),
                (4., 16.),
                (8., 20.),
            ]);
            line(&[(13.5, 6.5), (17.5, 10.5)]);
        }
        Icon::Trash => {
            line(&[(4., 7.), (20., 7.)]);
            line(&[(9., 7.), (9., 4.), (15., 4.), (15., 7.)]);
            line(&[(18., 7.), (17., 20.), (7., 20.), (6., 7.)]);
            line(&[(10., 11.), (10., 16.)]);
            line(&[(14., 11.), (14., 16.)]);
        }
        Icon::Copy => {
            p.rect_stroke(
                egui::Rect::from_min_max(q(8., 8.), q(19., 19.)),
                2.,
                stroke,
                egui::StrokeKind::Inside,
            );
            line(&[
                (16., 8.),
                (16., 6.),
                (14., 4.),
                (6., 4.),
                (4., 6.),
                (4., 14.),
                (6., 16.),
                (8., 16.),
            ]);
        }
        Icon::Save => {
            line(&[
                (5., 4.),
                (17., 4.),
                (19., 6.),
                (19., 20.),
                (5., 20.),
                (5., 4.),
            ]);
            line(&[(8., 4.), (8., 10.), (16., 10.), (16., 4.)]);
            line(&[(8., 20.), (8., 14.), (16., 14.), (16., 20.)]);
        }
        Icon::Folder => {
            line(&[
                (3., 7.),
                (3., 17.),
                (5., 19.),
                (19., 19.),
                (21., 17.),
                (21., 9.),
                (19., 7.),
                (12., 7.),
                (10., 5.),
                (5., 5.),
                (3., 7.),
            ]);
            p.circle_stroke(q(16.5, 13.5), rect.width() * 2.5 / 24., stroke);
            line(&[(18.3, 15.3), (20.5, 17.5)]);
        }
        Icon::Stack => {
            line(&[(4., 9.), (12., 4.), (20., 9.), (12., 14.), (4., 9.)]);
            line(&[(4., 13.), (12., 18.), (20., 13.)]);
            line(&[(4., 17.), (12., 22.), (20., 17.)]);
        }
    }
}

/// Pixel density of [`hover_blur_image`]: 2× the card, so HiDPI displays
/// sample it without upscaling.
const HOVER_BLUR_DENSITY: f64 = 2.;

/// The card's cover-cropped media, resized to 2× the card and blurred with the
/// shipping `blur(2px)` standard deviation. Built off the UI thread once per
/// card; the hover treatment fades it in over the sharp image.
pub fn hover_blur_image(image: &egui::ColorImage) -> Option<egui::ColorImage> {
    use captures_app::preview::{THUMBNAIL_CARD_HEIGHT, THUMBNAIL_PADDING, THUMBNAIL_WIDTH};
    let [width, height] = image.size;
    if width == 0 || height == 0 {
        return None;
    }
    let card = egui::vec2(
        (THUMBNAIL_WIDTH - THUMBNAIL_PADDING * 2.) as f32,
        THUMBNAIL_CARD_HEIGHT as f32,
    );
    let uv = cover_uv(egui::vec2(width as f32, height as f32), card);
    let crop_x = (uv.min.x * width as f32).round() as u32;
    let crop_y = (uv.min.y * height as f32).round() as u32;
    let crop_width = ((uv.width() * width as f32).round() as u32).clamp(1, width as u32 - crop_x);
    let crop_height =
        ((uv.height() * height as f32).round() as u32).clamp(1, height as u32 - crop_y);
    let pixels = image::RgbaImage::from_raw(width as u32, height as u32, image.as_raw().to_vec())?;
    let cropped = image::imageops::crop_imm(&pixels, crop_x, crop_y, crop_width, crop_height);
    let target = [
        (f64::from(card.x) * HOVER_BLUR_DENSITY) as u32,
        (f64::from(card.y) * HOVER_BLUR_DENSITY) as u32,
    ];
    let resized = image::imageops::resize(
        &*cropped,
        target[0],
        target[1],
        image::imageops::FilterType::Triangle,
    );
    let sigma = captures_app::preview_chrome::HOVER_MEDIA_BLUR * HOVER_BLUR_DENSITY;
    let blurred = image::imageops::fast_blur(&resized, sigma as f32);
    Some(egui::ColorImage::from_rgba_premultiplied(
        [target[0] as usize, target[1] as usize],
        blurred.as_raw(),
    ))
}

fn cover_uv(image: egui::Vec2, target: egui::Vec2) -> egui::Rect {
    let image_aspect = image.x / image.y.max(1.);
    let target_aspect = target.x / target.y.max(1.);
    if image_aspect > target_aspect {
        let visible = target_aspect / image_aspect;
        let inset = (1. - visible) / 2.;
        egui::Rect::from_min_max(egui::pos2(inset, 0.), egui::pos2(1. - inset, 1.))
    } else {
        let visible = image_aspect / target_aspect;
        let inset = (1. - visible) / 2.;
        egui::Rect::from_min_max(egui::pos2(0., inset), egui::pos2(1., 1. - inset))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_drop_shake_settles_at_420ms_and_obeys_reduced_motion() {
        assert_eq!(reject_offset(0., false), 0.);
        assert!((reject_offset(0.084, false) + 8.).abs() < 0.001);
        assert!((reject_offset(0.168, false) - 7.).abs() < 0.001);
        assert_ne!(reject_offset(0.419, false), 0.);
        assert_eq!(reject_offset(0.420, false), 0.);
        assert_eq!(reject_offset(1., false), 0.);
        assert_eq!(reject_offset(0.084, true), 0.);
    }

    fn raw(screen: egui::Rect, events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(screen),
            events,
            ..Default::default()
        }
    }

    fn pointer(pos: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    fn moved(pos: egui::Pos2) -> Vec<egui::Event> {
        vec![egui::Event::PointerMoved(pos)]
    }

    fn run_card(
        ctx: &egui::Context,
        texture: &egui::TextureHandle,
        events: Vec<egui::Event>,
        collapsed: bool,
        interactive: bool,
    ) -> Option<Action> {
        let pointer = events.iter().rev().find_map(|event| match event {
            egui::Event::PointerMoved(pos) => Some(*pos),
            _ => None,
        });
        run_card_on_desktop(
            ctx,
            texture,
            events,
            collapsed,
            interactive,
            egui::Pos2::ZERO,
            pointer,
        )
    }

    fn run_card_on_desktop(
        ctx: &egui::Context,
        texture: &egui::TextureHandle,
        events: Vec<egui::Event>,
        collapsed: bool,
        interactive: bool,
        origin: egui::Pos2,
        desktop_pointer: Option<egui::Pos2>,
    ) -> Option<Action> {
        run_card_state(
            ctx,
            texture,
            events,
            collapsed,
            interactive,
            origin,
            desktop_pointer,
            None,
            true,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn run_card_state(
        ctx: &egui::Context,
        texture: &egui::TextureHandle,
        events: Vec<egui::Event>,
        collapsed: bool,
        interactive: bool,
        origin: egui::Pos2,
        desktop_pointer: Option<egui::Pos2>,
        busy: Option<Busy>,
        can_save: bool,
        saved: bool,
    ) -> Option<Action> {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(284., 160.));
        let tokens = crate::tokens::load()["dark-mustard"].clone();
        let mut input = raw(screen, events);
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .outer_rect = Some(egui::Rect::from_min_size(origin, screen.size()));
        ctx.begin_pass(input);
        let mut ui = egui::Ui::new(
            ctx.clone(),
            egui::Id::unique(("preview-card-input-test", interactive)),
            egui::UiBuilder::new().max_rect(screen),
        );
        let action = show(
            &mut ui,
            &tokens,
            View {
                artifact_id: "artifact",
                texture,
                width: 320,
                height: 180,
                size_bytes: 245_760,
                clipboard_current: false,
                saved_feedback: false,
                busy,
                message: None,
                can_save,
                saved,
                interactive,
                collapsed,
                stack_count: 3,
                depth: usize::from(!interactive),
                desktop_pointer,
                reject_offset: 0.,
                right_anchor: false,
                top_anchor: false,
                blurred: None,
                editor: EditorPhase::Idle,
                editor_elapsed_ms: 0.,
                hover_locked: false,
                reduced_motion: false,
                highlight: 0.,
                warning: None,
                depth_shade: 1.,
            },
        );
        let mut output = ctx.end_pass();
        output.textures_delta.clear();
        action
    }

    #[test]
    fn shipping_action_labels_fit_asymmetric_284_by_160_card() {
        for (saved, right_anchor) in [(false, false), (true, false), (false, true), (true, true)] {
            let ctx = egui::Context::default();
            let tokens = crate::tokens::load()["dark-mustard"].clone();
            tokens.apply(&ctx, false);
            let texture = ctx.load_texture(
                "actions",
                egui::ColorImage::filled([2, 2], egui::Color32::WHITE),
                Default::default(),
            );
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(284., 160.));
            ctx.begin_pass(raw(rect, moved(rect.center())));
            let mut ui = egui::Ui::new(
                ctx.clone(),
                egui::Id::unique("action-layout"),
                egui::UiBuilder::new().max_rect(rect),
            );
            show(
                &mut ui,
                &tokens,
                View {
                    artifact_id: "saved",
                    texture: &texture,
                    width: 310,
                    height: 170,
                    size_bytes: 245_760,
                    clipboard_current: false,
                    saved_feedback: false,
                    busy: None,
                    message: None,
                    can_save: true,
                    saved,
                    interactive: true,
                    collapsed: false,
                    stack_count: 1,
                    depth: 0,
                    desktop_pointer: None,
                    reject_offset: 0.,
                    right_anchor,
                    top_anchor: false,
                    blurred: None,
                    editor: EditorPhase::Idle,
                    editor_elapsed_ms: 0.,
                    hover_locked: false,
                    reduced_motion: false,
                    highlight: 0.,
                    warning: None,
                    depth_shade: 1.,
                },
            );
            let mut output = ctx.end_pass();
            let labels = ["Copy", if saved { "Show in Folder" } else { "Save file" }];
            let bounds: Vec<_> = labels
                .iter()
                .map(|label| {
                    output
                        .shapes
                        .iter()
                        .find_map(|shape| match &shape.shape {
                            egui::Shape::Text(text) if text.galley.text() == *label => {
                                Some(text.galley.rect.translate(text.pos.to_vec2()))
                            }
                            _ => None,
                        })
                        .expect("every action must be painted")
                })
                .collect();
            assert!(
                bounds[0].max.y < bounds[1].min.y,
                "actions must be vertical: {bounds:?}"
            );
            assert!(bounds.iter().all(|bounds| rect.contains_rect(*bounds)));
            output.textures_delta.clear();
        }
    }

    #[test]
    fn chrome_hides_at_rest_reveals_on_focus_and_keeps_busy_primary_contrast() {
        let ctx = egui::Context::default();
        let tokens = crate::tokens::load()["dark-mustard"].clone();
        let texture = ctx.load_texture(
            "chrome",
            egui::ColorImage::filled([2, 2], Color32::WHITE),
            Default::default(),
        );
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(284., 160.));
        for (hovered, focused, busy) in [
            (false, false, None),
            (true, false, Some(Busy::Save)),
            (false, true, None),
        ] {
            ctx.begin_pass(raw(
                screen,
                moved(if hovered {
                    screen.center()
                } else {
                    egui::pos2(500., 500.)
                }),
            ));
            let mut ui = egui::Ui::new(
                ctx.clone(),
                egui::Id::unique("chrome-test"),
                egui::UiBuilder::new().max_rect(screen),
            );
            if focused {
                ui.memory_mut(|memory| {
                    memory.request_focus(ui.scope_id().with(("copy", "fixture")))
                });
            }
            show(
                &mut ui,
                &tokens,
                View {
                    artifact_id: "fixture",
                    texture: &texture,
                    width: 391,
                    height: 207,
                    size_bytes: 245_760,
                    clipboard_current: false,
                    saved_feedback: false,
                    busy,
                    message: None,
                    can_save: true,
                    saved: false,
                    interactive: true,
                    collapsed: false,
                    stack_count: 1,
                    depth: 0,
                    desktop_pointer: None,
                    reject_offset: 0.,
                    right_anchor: true,
                    top_anchor: false,
                    blurred: None,
                    editor: EditorPhase::Idle,
                    editor_elapsed_ms: 0.,
                    hover_locked: false,
                    reduced_motion: false,
                    highlight: 0.,
                    warning: None,
                    depth_shade: 1.,
                },
            );
            let mut output = ctx.end_pass();
            let text = |label: &str| {
                output.shapes.iter().find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == label => Some(text),
                    _ => None,
                })
            };
            assert_eq!(text("391 × 207 · 246 KB").is_some(), !hovered && !focused);
            assert_eq!(text("Copy").is_some(), hovered || focused);
            if hovered || focused {
                let save = text("Save file").unwrap();
                assert_eq!(
                    save.fallback_color,
                    Color32::from_rgb(23, 24, 27),
                    "busy yellow actions must not turn white"
                );
            }
            output.textures_delta.clear();
        }
    }

    #[test]
    fn tab_walks_card_controls_in_shipping_order_and_reveals_them() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let tokens = crate::tokens::load()["dark-mustard"].clone();
        let texture = ctx.load_texture(
            "tab",
            egui::ColorImage::filled([2, 2], Color32::WHITE),
            Default::default(),
        );
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(284., 160.));
        let tab = egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let frame = |events: Vec<egui::Event>| {
            // The pointer stays outside: only keyboard focus reveals chrome.
            let mut events = events;
            events.insert(0, egui::Event::PointerMoved(egui::pos2(500., 500.)));
            let mut output = ctx.run_ui(raw(screen, events), |ui| {
                show(
                    ui,
                    &tokens,
                    View {
                        artifact_id: "fixture",
                        texture: &texture,
                        width: 391,
                        height: 207,
                        size_bytes: 245_760,
                        clipboard_current: false,
                        saved_feedback: false,
                        busy: None,
                        message: None,
                        can_save: true,
                        saved: true,
                        interactive: true,
                        collapsed: false,
                        stack_count: 1,
                        depth: 0,
                        desktop_pointer: None,
                        reject_offset: 0.,
                        right_anchor: true,
                        top_anchor: false,
                        blurred: None,
                        editor: EditorPhase::Idle,
                        editor_elapsed_ms: 0.,
                        hover_locked: false,
                        reduced_motion: false,
                        highlight: 0.,
                        warning: None,
                        depth_shade: 1.,
                    },
                );
            });
            output.textures_delta.clear();
            let update = output.platform_output.accesskit_update.take().unwrap();
            let focused = update
                .nodes
                .iter()
                .find(|(id, _)| *id == update.focus)
                .and_then(|(_, node)| node.label().map(str::to_owned));
            let painted = |label: &str| {
                output.shapes.iter().any(|shape| {
                    matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == label)
                })
            };
            (focused, painted("Copy"), painted("391 × 207 · 246 KB"))
        };
        let (_, copy, size) = frame(vec![]);
        assert!(!copy && size, "idle chrome stays hidden");
        for label in ["Close", "Delete", "Edit", "Copy", "Show in Folder"] {
            frame(vec![tab.clone()]);
            let (focused, copy, size) = frame(vec![]);
            assert_eq!(focused.as_deref(), Some(label));
            assert!(copy && !size, "focus within the card reveals its controls");
        }
    }

    #[test]
    fn clipboard_owner_hides_copy_shows_chip_and_saved_confirms() {
        let ctx = egui::Context::default();
        let tokens = crate::tokens::load()["dark-mustard"].clone();
        let texture = ctx.load_texture(
            "clipboard-chip",
            egui::ColorImage::filled([2, 2], Color32::WHITE),
            Default::default(),
        );
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(284., 180.));
        for (clipboard_current, saved_feedback) in [(false, false), (true, false), (true, true)] {
            ctx.begin_pass(raw(screen, moved(screen.center())));
            let mut ui = egui::Ui::new(
                ctx.clone(),
                egui::Id::unique("clipboard-chip"),
                egui::UiBuilder::new().max_rect(screen),
            );
            show(
                &mut ui,
                &tokens,
                View {
                    artifact_id: "fixture",
                    texture: &texture,
                    width: 391,
                    height: 207,
                    size_bytes: 245_760,
                    clipboard_current,
                    saved_feedback,
                    busy: None,
                    message: None,
                    can_save: true,
                    saved: saved_feedback,
                    interactive: true,
                    collapsed: false,
                    stack_count: 1,
                    depth: 0,
                    desktop_pointer: None,
                    reject_offset: 0.,
                    right_anchor: false,
                    top_anchor: false,
                    blurred: None,
                    editor: EditorPhase::Idle,
                    editor_elapsed_ms: 0.,
                    hover_locked: false,
                    // The chip's arrival starts transparent; settle it.
                    reduced_motion: true,
                    highlight: 0.,
                    warning: None,
                    depth_shade: 1.,
                },
            );
            let mut output = ctx.end_pass();
            let has = |label: &str| {
                output.shapes.iter().any(|shape| {
                    matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == label)
                })
            };
            assert_eq!(has("Copy"), !clipboard_current);
            assert_eq!(has("Copied to clipboard"), clipboard_current);
            assert_eq!(has("Saved"), saved_feedback);
            assert_eq!(has("Save file"), !saved_feedback);
            output.textures_delta.clear();
        }
    }

    #[test]
    fn image_drag_starts_on_image_press_but_never_on_action_press() {
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "drag-crossing",
            egui::ColorImage::filled([2, 2], Color32::WHITE),
            Default::default(),
        );
        let origin = egui::pos2(40., 60.);
        run_card(&ctx, &texture, moved(origin), false, true);
        assert_eq!(
            run_card(&ctx, &texture, pointer(origin, true), false, true),
            Some(Action::DragFile)
        );
        assert_eq!(
            run_card(
                &ctx,
                &texture,
                pointer(egui::pos2(142., 60.), false),
                false,
                true
            ),
            None
        );
        run_card(&ctx, &texture, moved(egui::pos2(142., 60.)), false, true);
        assert_eq!(
            run_card(
                &ctx,
                &texture,
                pointer(egui::pos2(142., 60.), true),
                false,
                true
            ),
            None
        );
        assert_eq!(
            run_card(
                &ctx,
                &texture,
                pointer(egui::pos2(142., 60.), false),
                false,
                true
            ),
            Some(Action::Copy)
        );
    }

    #[test]
    fn image_drag_starts_right_after_pointer_rested_on_an_action() {
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "drag-after-action",
            egui::ColorImage::filled([2, 2], Color32::WHITE),
            Default::default(),
        );
        // The previous frame only saw the pointer over Copy; the move to the
        // image and the press then arrive together in one frame.
        run_card(&ctx, &texture, moved(egui::pos2(142., 60.)), false, true);
        assert_eq!(
            run_card(
                &ctx,
                &texture,
                pointer(egui::pos2(40., 60.), true),
                false,
                true
            ),
            Some(Action::DragFile)
        );
    }

    #[test]
    fn second_button_saves_unsaved_reveals_saved_and_is_guarded_while_busy() {
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "save-reveal",
            egui::ColorImage::filled([2, 2], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        let click = egui::pos2(142., 100.);
        for (saved, busy, can_save, expected) in [
            (false, None, true, Some(Action::Save)),
            (true, None, false, Some(Action::Reveal)),
            (false, None, false, None),
            (true, Some(Busy::Reveal), false, None),
        ] {
            run_card_state(
                &ctx,
                &texture,
                moved(click),
                false,
                true,
                egui::Pos2::ZERO,
                Some(click),
                busy,
                can_save,
                saved,
            );
            run_card_state(
                &ctx,
                &texture,
                pointer(click, true),
                false,
                true,
                egui::Pos2::ZERO,
                Some(click),
                busy,
                can_save,
                saved,
            );
            assert_eq!(
                run_card_state(
                    &ctx,
                    &texture,
                    pointer(click, false),
                    false,
                    true,
                    egui::Pos2::ZERO,
                    Some(click),
                    busy,
                    can_save,
                    saved,
                ),
                expected
            );
        }
    }

    fn run_controls(ctx: &egui::Context, events: Vec<egui::Event>) -> Option<StackAction> {
        let screen = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(
                captures_app::preview::THUMBNAIL_WIDTH as f32,
                captures_app::preview::THUMBNAIL_CONTROL_GUTTER as f32,
            ),
        );
        let tokens = crate::tokens::load()["dark-mustard"].clone();
        ctx.begin_pass(raw(screen, events));
        let mut ui = egui::Ui::new(
            ctx.clone(),
            egui::Id::unique("preview-controls-input-test"),
            egui::UiBuilder::new().max_rect(screen),
        );
        let action = show_stack_controls(&mut ui, &tokens, false, false, false);
        let mut output = ctx.end_pass();
        output.textures_delta.clear();
        action
    }

    fn painted_texts(output: &egui::FullOutput) -> Vec<String> {
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn minimize_swaps_icon_for_show_less_label_on_hover_and_grows_inward() {
        for right_anchor in [false, true] {
            let ctx = egui::Context::default();
            let tokens = crate::tokens::load()["dark-mustard"].clone();
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(284., 52.));
            // Resting minimize control: 32..60 left-anchored, 224..252 right.
            let pointer = egui::pos2(if right_anchor { 238. } else { 46. }, 26.);
            let mut label_rect = None;
            for (frame, position) in [(0, egui::pos2(500., 500.)), (1, pointer), (2, pointer)] {
                ctx.begin_pass(raw(screen, moved(position)));
                let mut ui = egui::Ui::new(
                    ctx.clone(),
                    egui::Id::unique("show-less-test"),
                    egui::UiBuilder::new().max_rect(screen),
                );
                show_stack_controls(&mut ui, &tokens, right_anchor, false, false);
                let mut output = ctx.end_pass();
                let texts = painted_texts(&output);
                assert_eq!(
                    texts.iter().any(|text| text == "Show less"),
                    frame == 2,
                    "frame {frame}: {texts:?}"
                );
                if frame == 2 {
                    label_rect = output.shapes.iter().find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) if text.galley.text() == "Show less" => {
                            Some(text.galley.rect.translate(text.pos.to_vec2()))
                        }
                        _ => None,
                    });
                }
                output.textures_delta.clear();
            }
            let label = label_rect.unwrap();
            // The 92 px pill keeps the Clear all square (outer edge) uncovered.
            if right_anchor {
                assert!(label.max.x <= 252. && label.min.x >= 160., "{label:?}");
            } else {
                assert!(label.min.x >= 32. && label.max.x <= 124., "{label:?}");
            }
        }
    }

    #[test]
    fn show_less_pill_widens_over_the_shipping_morph() {
        let ctx = egui::Context::default();
        let tokens = crate::tokens::load()["dark-mustard"].clone();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(284., 52.));
        let pointer = egui::pos2(46., 26.);
        let mut widths = Vec::new();
        for (time, position) in [
            (0., egui::pos2(500., 500.)),
            (0.01, pointer),
            (0.05, pointer),
            (0.4, pointer),
        ] {
            ctx.begin_pass(egui::RawInput {
                time: Some(time),
                ..raw(screen, moved(position))
            });
            let mut ui = egui::Ui::new(
                ctx.clone(),
                egui::Id::unique("show-less-morph-test"),
                egui::UiBuilder::new().max_rect(screen),
            );
            show_stack_controls(&mut ui, &tokens, false, false, false);
            let mut output = ctx.end_pass();
            // The pill is the widest glass rect starting at the inner slot.
            let width = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Rect(rect) if (rect.rect.min.x - 30.).abs() < 0.5 => {
                        Some(rect.rect.width())
                    }
                    _ => None,
                })
                .fold(0., f32::max);
            widths.push(width);
            output.textures_delta.clear();
        }
        assert_eq!(widths[0], 28.);
        assert!(widths[2] > 28. && widths[2] < 92., "mid-morph {widths:?}");
        assert_eq!(widths[3], 92.);
    }

    #[test]
    fn warning_chip_follows_metadata_and_exits_paint_without_chrome() {
        let ctx = egui::Context::default();
        let tokens = crate::tokens::load()["dark-mustard"].clone();
        let texture = ctx.load_texture(
            "warning-exit",
            egui::ColorImage::filled([4, 4], Color32::WHITE),
            Default::default(),
        );
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(284., 160.));
        ctx.begin_pass(raw(screen, moved(egui::pos2(900., 900.))));
        let mut ui = egui::Ui::new(
            ctx.clone(),
            egui::Id::unique("warning-test"),
            egui::UiBuilder::new().max_rect(screen),
        );
        show(
            &mut ui,
            &tokens,
            View {
                artifact_id: "warning",
                texture: &texture,
                width: 320,
                height: 180,
                size_bytes: 1_024,
                clipboard_current: false,
                saved_feedback: false,
                busy: None,
                message: None,
                can_save: true,
                saved: false,
                interactive: true,
                collapsed: false,
                stack_count: 1,
                depth: 0,
                desktop_pointer: None,
                reject_offset: 0.,
                right_anchor: false,
                top_anchor: false,
                blurred: None,
                editor: EditorPhase::Idle,
                editor_elapsed_ms: 0.,
                hover_locked: false,
                reduced_motion: false,
                highlight: 1.,
                warning: Some(preview_chrome::WARNING_CLIPBOARD_UNAVAILABLE),
                depth_shade: 1.,
            },
        );
        let mut output = ctx.end_pass();
        let texts = painted_texts(&output);
        assert!(
            texts.iter().any(|text| text == "Clipboard unavailable"),
            "{texts:?}"
        );
        output.textures_delta.clear();

        let particles =
            captures_app::preview_motion::dust_particles(284., 160., (320., 180.), (22.5, 22.5), 1);
        for (kind, elapsed, mesh) in [
            (captures_app::preview_motion::ExitKind::Dust, 1_000., true),
            (captures_app::preview_motion::ExitKind::Dismiss, 100., false),
        ] {
            ctx.begin_pass(raw(screen, Vec::new()));
            let mut ui = egui::Ui::new(
                ctx.clone(),
                egui::Id::unique("exit-test"),
                egui::UiBuilder::new().max_rect(screen),
            );
            assert!(show_exit(
                &mut ui,
                &tokens,
                screen,
                ExitView {
                    texture: &texture,
                    blurred: None,
                    kind,
                    elapsed_ms: elapsed,
                    dust: &particles,
                    right_anchor: false,
                    reduced_motion: false,
                },
            ));
            let mut output = ctx.end_pass();
            assert!(painted_texts(&output).is_empty(), "exits paint no chrome");
            let chips = output.shapes.iter().any(
                |shape| matches!(&shape.shape, egui::Shape::Mesh(mesh) if mesh.vertices.len() > 4),
            );
            assert_eq!(chips, mesh, "{kind:?}");
            output.textures_delta.clear();
        }
    }

    fn run_cues(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        overflow: captures_app::preview::StackOverflow,
        top_anchor: bool,
    ) -> (Option<i32>, egui::FullOutput) {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(340., 600.));
        let tokens = crate::tokens::load()["dark-mustard"].clone();
        ctx.begin_pass(raw(screen, events));
        let mut ui = egui::Ui::new(
            ctx.clone(),
            egui::Id::unique("overflow-cue-test"),
            egui::UiBuilder::new().max_rect(screen),
        );
        let slots = show_overflow_cues(&mut ui, &tokens, screen, overflow, top_anchor);
        let mut output = ctx.end_pass();
        output.textures_delta.clear();
        (slots, output)
    }

    #[test]
    fn overflow_cues_only_show_hidden_edges_and_scroll_one_slot() {
        use captures_app::preview::StackOverflow;
        let ctx = egui::Context::default();
        let (_, output) = run_cues(&ctx, vec![], StackOverflow::default(), false);
        assert!(
            !output
                .shapes
                .iter()
                .any(|shape| matches!(shape.shape, egui::Shape::Rect(_))),
            "no overflow paints no cues"
        );
        let both = StackOverflow {
            above: true,
            below: true,
        };
        for (point, expected) in [(egui::pos2(170., 17.), -1), (egui::pos2(170., 583.), 1)] {
            run_cues(&ctx, moved(point), both, false);
            run_cues(&ctx, pointer(point, true), both, false);
            assert_eq!(
                run_cues(&ctx, pointer(point, false), both, false).0,
                Some(expected)
            );
        }
        let above_only = StackOverflow {
            above: true,
            below: false,
        };
        let point = egui::pos2(170., 583.);
        run_cues(&ctx, moved(point), above_only, false);
        run_cues(&ctx, pointer(point, true), above_only, false);
        assert_eq!(
            run_cues(&ctx, pointer(point, false), above_only, false).0,
            None,
            "a hidden cue must not scroll"
        );
    }

    #[test]
    fn only_compact_rear_images_receive_the_glass_depth_overlay() {
        for (collapsed, depth, expected) in [(true, 0, false), (true, 1, true), (false, 1, false)] {
            let ctx = egui::Context::default();
            let texture = ctx.load_texture(
                "shade",
                egui::ColorImage::filled([2, 2], egui::Color32::RED),
                egui::TextureOptions::LINEAR,
            );
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(284., 160.));
            let tokens = crate::tokens::load()["dark-mustard"].clone();
            ctx.begin_pass(raw(screen, vec![]));
            let mut ui = egui::Ui::new(
                ctx.clone(),
                egui::Id::unique("shade-test"),
                egui::UiBuilder::new().max_rect(screen),
            );
            show(
                &mut ui,
                &tokens,
                View {
                    artifact_id: "red",
                    texture: &texture,
                    width: 2,
                    height: 2,
                    size_bytes: 245_760,
                    clipboard_current: false,
                    saved_feedback: false,
                    busy: None,
                    message: None,
                    can_save: true,
                    saved: false,
                    interactive: depth == 0,
                    collapsed,
                    stack_count: 2,
                    depth,
                    desktop_pointer: None,
                    reject_offset: 0.,
                    right_anchor: false,
                    top_anchor: false,
                    blurred: None,
                    editor: EditorPhase::Idle,
                    editor_elapsed_ms: 0.,
                    hover_locked: false,
                    reduced_motion: false,
                    highlight: 0.,
                    warning: None,
                    depth_shade: 1.,
                },
            );
            let mut output = ctx.end_pass();
            // CSS depth1=.13748, glass-strong-solid=(15,15,18). Premultiplied
            // 8-bit overlay rounds to (2,2,2,35), not black or full opacity.
            let shade = egui::Color32::from_rgba_premultiplied(2, 2, 2, 35);
            assert_eq!(
                output.shapes.iter().any(|shape| matches!(&shape.shape,
                egui::Shape::Rect(rect) if rect.fill == shade)),
                expected
            );
            output.textures_delta.clear();
        }
    }

    struct ChromeCase {
        pointer: egui::Pos2,
        editor: EditorPhase,
        hover_locked: bool,
        top_anchor: bool,
        blurred: bool,
    }

    /// One card frame at 284×160 with reduced motion so transitions land at
    /// once. Returns painted texts, the output and the blurred texture id.
    fn chrome_frame(
        ctx: &egui::Context,
        case: &ChromeCase,
    ) -> (Vec<String>, egui::FullOutput, egui::TextureId) {
        let tokens = crate::tokens::load()["dark-mustard"].clone();
        let texture = ctx.load_texture(
            "chrome-sharp",
            egui::ColorImage::filled([4, 4], Color32::WHITE),
            Default::default(),
        );
        let blurred = ctx.load_texture(
            "chrome-blurred",
            egui::ColorImage::filled([4, 4], Color32::GRAY),
            Default::default(),
        );
        // Room above and below the card for tooltips.
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(284., 240.));
        ctx.begin_pass(raw(screen, moved(case.pointer)));
        let mut ui = egui::Ui::new(
            ctx.clone(),
            egui::Id::unique("chrome-frame"),
            egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                egui::pos2(0., 40.),
                egui::vec2(284., 160.),
            )),
        );
        show(
            &mut ui,
            &tokens,
            View {
                artifact_id: "fixture",
                texture: &texture,
                width: 391,
                height: 207,
                size_bytes: 245_760,
                clipboard_current: false,
                saved_feedback: false,
                busy: None,
                message: None,
                can_save: true,
                saved: true,
                interactive: true,
                collapsed: false,
                stack_count: 1,
                depth: 0,
                desktop_pointer: None,
                reject_offset: 0.,
                right_anchor: false,
                top_anchor: case.top_anchor,
                blurred: case.blurred.then_some(&blurred),
                editor: case.editor,
                editor_elapsed_ms: 1_000.,
                hover_locked: case.hover_locked,
                reduced_motion: true,
                highlight: 0.,
                warning: None,
                depth_shade: 1.,
            },
        );
        let mut output = ctx.end_pass();
        output.textures_delta.clear();
        (painted_texts(&output), output, blurred.id())
    }

    fn case(pointer: egui::Pos2) -> ChromeCase {
        ChromeCase {
            pointer,
            editor: EditorPhase::Idle,
            hover_locked: false,
            top_anchor: false,
            blurred: true,
        }
    }

    fn text_rect(output: &egui::FullOutput, label: &str) -> Option<egui::Rect> {
        output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == label => {
                Some(text.galley.rect.translate(text.pos.to_vec2()))
            }
            _ => None,
        })
    }

    fn uses_texture(output: &egui::FullOutput, id: egui::TextureId) -> bool {
        output.shapes.iter().any(
            |shape| matches!(&shape.shape, egui::Shape::Rect(rect) if rect.fill_texture_id() == id),
        )
    }

    #[test]
    fn hover_blurs_and_darkens_media_but_the_stale_pointer_lock_holds_it_off() {
        let center = egui::pos2(142., 120.);
        for locked in [false, true] {
            let ctx = egui::Context::default();
            let mut hover = case(center);
            hover.hover_locked = locked;
            chrome_frame(&ctx, &hover);
            let (texts, mut output, blurred) = chrome_frame(&ctx, &hover);
            assert_eq!(
                texts.iter().any(|text| text == "Copy"),
                !locked,
                "{texts:?}"
            );
            assert_eq!(
                texts.iter().any(|text| text == "391 × 207 · 246 KB"),
                locked
            );
            assert_eq!(uses_texture(&output, blurred), !locked);
            // The sharp media scales 1.015 and darkens to brightness .5.
            let sharp = output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect)
                    if rect.fill_texture_id() != blurred
                        && rect.fill_texture_id() != egui::TextureId::default() =>
                {
                    Some(rect.clone())
                }
                _ => None,
            });
            let sharp = sharp.expect("sharp media");
            if locked {
                assert_eq!(sharp.rect.width(), 284.);
                assert_eq!(sharp.fill, Color32::WHITE);
            } else {
                assert!((sharp.rect.width() - 284. * 1.015).abs() < 0.01);
                assert_eq!(
                    sharp.fill,
                    Color32::from_rgba_premultiplied(128, 128, 128, 255)
                );
            }
            output.textures_delta.clear();
        }
    }

    #[test]
    fn icon_tooltips_are_instant_glass_chips_on_icons_only() {
        // Delete sits at (42..70, 48..76) on this saved, left-anchored card.
        for top_anchor in [false, true] {
            let ctx = egui::Context::default();
            let mut over_delete = case(egui::pos2(56., 62.));
            over_delete.top_anchor = top_anchor;
            chrome_frame(&ctx, &over_delete);
            let (_, mut output, _) = chrome_frame(&ctx, &over_delete);
            let tip = text_rect(&output, "Delete").expect("Delete tooltip");
            assert!((tip.center().x - 56.).abs() < 1., "{tip:?}");
            if top_anchor {
                assert!(tip.min.y > 76., "top stacks open tips below: {tip:?}");
            } else {
                assert!(tip.max.y < 48., "bottom stacks open tips above: {tip:?}");
            }
            output.textures_delta.clear();
        }
        let ctx = egui::Context::default();
        let over_copy = case(egui::pos2(142., 100.));
        chrome_frame(&ctx, &over_copy);
        let (texts, mut output, _) = chrome_frame(&ctx, &over_copy);
        assert_eq!(texts.iter().filter(|text| *text == "Copy").count(), 1);
        assert!(!texts.iter().any(|text| text.contains("Drag the original")));
        output.textures_delta.clear();
    }

    #[test]
    fn editor_presence_pins_the_pill_and_ring_and_offers_show_in_editor_on_hover() {
        let accent = crate::tokens::load()["dark-mustard"].color("theme-accent");
        let ring = |output: &egui::FullOutput| {
            output.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Rect(rect)
                    if rect.stroke.width == 2. && rect.stroke.color == accent.gamma_multiply(0.9))
            })
        };
        let ctx = egui::Context::default();
        let mut away = case(egui::pos2(500., 500.));
        away.editor = EditorPhase::Present;
        chrome_frame(&ctx, &away);
        let (texts, mut output, _) = chrome_frame(&ctx, &away);
        assert!(texts.iter().any(|text| text == "In editor"), "{texts:?}");
        assert!(
            !texts.iter().any(|text| text == "Copy"),
            "chrome stays idle"
        );
        assert!(ring(&output));
        let pill = text_rect(&output, "In editor").unwrap();
        assert!(pill.max.x <= 276. && pill.min.x > 150., "{pill:?}");
        output.textures_delta.clear();

        let mut hover = case(egui::pos2(pill.center().x, pill.center().y));
        hover.editor = EditorPhase::Present;
        hover.hover_locked = true;
        chrome_frame(&ctx, &hover);
        let (texts, mut output, _) = chrome_frame(&ctx, &hover);
        assert!(
            texts.iter().any(|text| text == "Show in editor"),
            "{texts:?}"
        );
        assert!(
            !texts.iter().any(|text| text == "Edit"),
            "the pill carries no tooltip"
        );
        output.textures_delta.clear();

        for phase in [EditorPhase::Idle, EditorPhase::Lingering] {
            let ctx = egui::Context::default();
            let mut rest = case(egui::pos2(500., 500.));
            rest.editor = phase;
            chrome_frame(&ctx, &rest);
            let (_, mut output, _) = chrome_frame(&ctx, &rest);
            assert!(!ring(&output));
            let tokens = crate::tokens::load()["dark-mustard"].clone();
            let compact = output.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Rect(rect)
                    if rect.rect.width() == 28. && rect.rect.min.x == 248.
                        && rect.fill == tokens.color("glass-strong"))
            });
            assert_eq!(compact, phase == EditorPhase::Lingering, "{phase:?}");
            output.textures_delta.clear();
        }
    }

    #[test]
    fn hover_blur_copy_is_card_sized_cover_cropped_and_softened() {
        let mut image = egui::ColorImage::filled([800, 200], Color32::BLACK);
        for y in 0..200 {
            for x in 400..800 {
                image.pixels[y * 800 + x] = Color32::WHITE;
            }
        }
        let blurred = hover_blur_image(&image).unwrap();
        assert_eq!(blurred.size, [568, 320]);
        // The hard black/white edge at the centre is spread across pixels.
        let row = 160 * 568;
        let edge = blurred.pixels[row + 284];
        assert!(edge.r() > 20 && edge.r() < 235, "{edge:?}");
        assert_eq!(blurred.pixels[row].r(), 0);
        assert_eq!(blurred.pixels[row + 567].r(), 255);
        assert!(hover_blur_image(&egui::ColorImage::filled([0, 0], Color32::BLACK)).is_none());
    }

    #[test]
    fn cover_crop_is_centered_and_preserves_aspect() {
        let wide = cover_uv(egui::vec2(400., 100.), egui::vec2(200., 100.));
        assert_eq!(wide.min, egui::pos2(0.25, 0.));
        assert_eq!(wide.max, egui::pos2(0.75, 1.));

        let tall = cover_uv(egui::vec2(100., 400.), egui::vec2(100., 200.));
        assert_eq!(tall.min, egui::pos2(0., 0.25));
        assert_eq!(tall.max, egui::pos2(1., 0.75));
    }

    #[test]
    fn stack_toolbar_only_appears_for_expanded_multi_card_stacks() {
        assert!(!stack_controls_visible(0, false));
        assert!(!stack_controls_visible(1, false));
        assert!(stack_controls_visible(2, false));
        assert!(!stack_controls_visible(2, true));
    }

    #[test]
    fn compact_drag_moves_without_expanding_and_next_click_still_expands() {
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "drag",
            egui::ColorImage::filled([2, 2], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        let press = egui::pos2(100., 80.);
        run_card(&ctx, &texture, moved(press), true, true);
        run_card(&ctx, &texture, pointer(press, true), true, true);
        assert_eq!(
            run_card(&ctx, &texture, moved(egui::pos2(142., 53.)), true, true),
            Some(Action::MoveStack(egui::pos2(42., -27.)))
        );
        // The window moved but winit still reports the old local pointer. A
        // fresh desktop sample stays put; do not drift or repaint in a loop.
        assert_eq!(
            run_card_on_desktop(
                &ctx,
                &texture,
                vec![],
                true,
                true,
                egui::pos2(42., -27.),
                Some(egui::pos2(142., 53.))
            ),
            None
        );
        assert_eq!(
            run_card_on_desktop(
                &ctx,
                &texture,
                moved(egui::pos2(110., 90.)),
                true,
                true,
                egui::pos2(42., -27.),
                Some(egui::pos2(152., 63.))
            ),
            Some(Action::MoveStack(egui::pos2(52., -17.)))
        );
        assert_eq!(
            run_card(
                &ctx,
                &texture,
                pointer(egui::pos2(142., 53.), false),
                true,
                true
            ),
            None
        );
        run_card(&ctx, &texture, pointer(press, true), true, true);
        assert_eq!(
            run_card(&ctx, &texture, pointer(press, false), true, true),
            Some(Action::ExpandStack)
        );
    }

    #[test]
    fn raw_input_routes_expanded_card_action_and_collapsed_front_expansion() {
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "preview-input-test",
            egui::ColorImage::new([2, 2], vec![egui::Color32::WHITE; 4]),
            egui::TextureOptions::LINEAR,
        );

        assert_eq!(
            run_card(&ctx, &texture, moved(egui::pos2(142., 60.)), false, true),
            None
        );
        assert_eq!(
            run_card(
                &ctx,
                &texture,
                pointer(egui::pos2(142., 60.), true),
                false,
                true
            ),
            None
        );
        assert_eq!(
            run_card(
                &ctx,
                &texture,
                pointer(egui::pos2(142., 60.), false),
                false,
                true
            ),
            Some(Action::Copy)
        );
        assert_eq!(
            run_card(&ctx, &texture, moved(egui::pos2(100., 80.)), true, false),
            None
        );
        assert_eq!(
            run_card(
                &ctx,
                &texture,
                pointer(egui::pos2(100., 80.), true),
                true,
                false
            ),
            None,
            "rear collapsed cards must be inert"
        );
        assert_eq!(
            run_card(
                &ctx,
                &texture,
                pointer(egui::pos2(100., 80.), false),
                true,
                false
            ),
            None
        );
        assert_eq!(
            run_card(&ctx, &texture, moved(egui::pos2(100., 80.)), true, true),
            None
        );
        assert_eq!(
            run_card(
                &ctx,
                &texture,
                pointer(egui::pos2(100., 80.), true),
                true,
                true
            ),
            None
        );
        assert_eq!(
            run_card(
                &ctx,
                &texture,
                pointer(egui::pos2(100., 80.), false),
                true,
                true
            ),
            Some(Action::ExpandStack)
        );
    }

    #[test]
    fn raw_input_routes_show_less_and_clear_all_controls() {
        let ctx = egui::Context::default();
        assert_eq!(run_controls(&ctx, moved(egui::pos2(40., 24.))), None);
        assert_eq!(
            run_controls(&ctx, pointer(egui::pos2(40., 24.), true)),
            None
        );
        assert_eq!(
            run_controls(&ctx, pointer(egui::pos2(40., 24.), false)),
            Some(StackAction::ToggleCollapsed)
        );
        assert_eq!(run_controls(&ctx, moved(egui::pos2(10., 24.))), None);
        assert_eq!(
            run_controls(&ctx, pointer(egui::pos2(10., 24.), true)),
            None
        );
        assert_eq!(
            run_controls(&ctx, pointer(egui::pos2(10., 24.), false)),
            Some(StackAction::ClearAll)
        );
    }
}
