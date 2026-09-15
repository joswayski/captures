mod layout;
pub(crate) mod media;
mod native_drag;
#[cfg(target_os = "linux")]
mod xdnd;

use crate::{
    Launch,
    effects::{Dissolve, HEIGHT, PAD, WIDTH},
    preferences::settings,
    theme::{Theme, font},
};
use gpui::{prelude::*, *};
use layout::{Anchor, CARD_HEIGHT, CARD_WIDTH, Placement, Side};
use media::{MediaKind, PreviewMedia};
use std::{fs, path::PathBuf, sync::Arc, time::Instant};

struct PreviewRegistry(WindowHandle<Preview>);
impl Global for PreviewRegistry {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExitKind {
    Dismiss,
    Delete,
}

struct Exit {
    kind: ExitKind,
    started: Instant,
    delay_ms: u64,
    dissolve: Option<Dissolve>,
}

struct Artifact {
    id: u64,
    media: PreviewMedia,
    saved_path: Option<PathBuf>,
    arrived: Instant,
    exit: Option<Exit>,
}

#[derive(Clone)]
struct DraggedCapture {
    id: u64,
    source: PathBuf,
    poster: Arc<PathBuf>,
}

struct DragGhost(DraggedCapture);
impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(142.))
            .h(px(80.))
            .rounded(px(8.))
            .overflow_hidden()
            .opacity(0.82)
            .shadow_lg()
            .child(
                img(self.0.poster.as_path())
                    .size_full()
                    .object_fit(ObjectFit::Cover),
            )
    }
}

#[derive(Clone, Copy)]
enum LineIcon {
    Check,
    ChevronDown,
    Close,
    Copy,
    Edit,
    Folder,
    Save,
    Trash,
}

fn line_icon(icon: LineIcon, color: &str) -> Img {
    let body = match icon {
        LineIcon::Check => r#"<path d="m5 12 4 4L19 6"/>"#,
        LineIcon::ChevronDown => r#"<path d="m6 9 6 6 6-6"/>"#,
        LineIcon::Close => r#"<path d="m6 6 12 12M18 6 6 18"/>"#,
        LineIcon::Copy => {
            r#"<rect x="8" y="8" width="11" height="11" rx="2"/><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2"/>"#
        }
        LineIcon::Edit => r#"<path d="m4 16-1 5 5-1L19 9l-4-4ZM13.5 6.5l4 4M4 16l4 4"/>"#,
        LineIcon::Folder => {
            r#"<path d="M3 7a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z"/><circle cx="16.5" cy="13.5" r="2.5"/><path d="m18.3 15.3 2.2 2.2"/>"#
        }
        LineIcon::Save => r#"<path d="M5 4h12l2 2v14H5Z"/><path d="M8 4v6h8V4M8 20v-6h8v6"/>"#,
        LineIcon::Trash => r#"<path d="M4 7h16M9 7V4h6v3m3 0-1 13H7L6 7m4 4v5m4-5v5"/>"#,
    };
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="{color}" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">{body}</svg>"#
    );
    img(Arc::new(Image::from_bytes(
        ImageFormat::Svg,
        svg.into_bytes(),
    )))
    .w(px(16.))
    .h(px(16.))
}

fn button_content(label: &'static str) -> (Option<LineIcon>, &'static str) {
    match label {
        "×" => (Some(LineIcon::Close), ""),
        "⌫" => (Some(LineIcon::Trash), ""),
        "✎" => (Some(LineIcon::Edit), ""),
        "▣  Copy" => (Some(LineIcon::Copy), "Copy"),
        "✓  Saved" => (Some(LineIcon::Check), "Saved"),
        "□  Show in Folder" => (Some(LineIcon::Folder), "Show in Folder"),
        "⇩  Save file" => (Some(LineIcon::Save), "Save file"),
        "⌄  Show less" => (Some(LineIcon::ChevronDown), "Show less"),
        "×  Clear all" => (Some(LineIcon::Close), "Clear all"),
        _ => (None, label),
    }
}

struct Preview {
    launch: Launch,
    artifacts: Vec<Artifact>,
    placement: Placement,
    collapsed: bool,
    transition: Option<Instant>,
    hovered: Option<u64>,
    stack_hovered: bool,
    copied: Option<u64>,
    saved_feedback: Option<(u64, Instant)>,
    saving: Option<u64>,
    rejected: Option<(u64, Instant)>,
    status: String,
    next_id: u64,
    reduced_motion: bool,
}

/// Add a completed capture to the existing mini-preview stack. The first push
/// creates the one shared stack window; subsequent pushes update that window.
pub fn push(launch: Launch, cx: &mut App) -> anyhow::Result<()> {
    let Some(path) = launch.path.clone() else {
        return if launch.mock {
            open(launch, cx)
        } else {
            Err(anyhow::anyhow!("mini preview push requires launch.path"))
        };
    };
    if let Some(handle) = cx
        .try_global::<PreviewRegistry>()
        .map(|registry| registry.0)
        && handle
            .update(cx, |preview, window, cx| {
                preview.add(path.clone(), window, cx)
            })
            .is_ok()
    {
        return Ok(());
    }
    open(launch, cx)
}

/// Return the stack window for capture-exclusion integration.
pub fn windows(cx: &App) -> Vec<AnyWindowHandle> {
    cx.try_global::<PreviewRegistry>()
        .filter(|registry| registry.0.read(cx).is_ok())
        .map(|registry| vec![registry.0.into()])
        .unwrap_or_default()
}

/// Open the standalone CLI fixture. Unlike `push`, this can seed the stack
/// from the profile when no explicit path is supplied.
pub fn open(launch: Launch, cx: &mut App) -> anyhow::Result<()> {
    let settings = settings::load(&launch.profile)?;
    let placement = Placement::parse(&settings.mini_preview_placement);
    let paths = fixture_paths(&launch)?;
    let display = cx
        .primary_display()
        .ok_or_else(|| anyhow::anyhow!("no display is available for mini previews"))?;
    let bounds = layout::window_bounds(display.bounds(), placement);
    let preview = Preview::new(launch.clone(), paths, placement)?;
    let handle = cx.open_window(
        WindowOptions {
            app_id: Some("captures-gpui-preview".into()),
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: launch.mock.then(|| TitlebarOptions {
                title: Some("Captures Preview fixture".into()),
                ..Default::default()
            }),
            focus: launch.mock,
            window_background: if launch.mock {
                WindowBackgroundAppearance::Opaque
            } else {
                WindowBackgroundAppearance::Transparent
            },
            kind: if launch.mock {
                WindowKind::Normal
            } else {
                WindowKind::PopUp
            },
            is_movable: true,
            is_resizable: false,
            is_minimizable: false,
            ..Default::default()
        },
        |_, cx| cx.new(|_| preview),
    )?;
    #[cfg(target_os = "linux")]
    if !launch.mock {
        crate::integration::configure_x11_floating("captures-gpui-preview", bounds)?;
    }
    cx.set_global(PreviewRegistry(handle));
    Ok(())
}

fn fixture_paths(launch: &Launch) -> anyhow::Result<Vec<PathBuf>> {
    if let Some(path) = &launch.path {
        return Ok(vec![path.clone(); if launch.mock { 3 } else { 1 }]);
    }
    let mut paths = fs::read_dir(launch.profile.join("captures"))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| {
                    matches!(
                        value.to_ascii_lowercase().as_str(),
                        "png" | "jpg" | "jpeg" | "webp" | "gif" | "mp4" | "mov" | "mkv" | "webm"
                    )
                })
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths.reverse();
    paths.truncate(8);
    Ok(paths)
}

impl Preview {
    fn new(launch: Launch, paths: Vec<PathBuf>, placement: Placement) -> anyhow::Result<Self> {
        let profile = launch.profile.clone();
        let artifacts = paths
            .into_iter()
            .enumerate()
            .map(|(index, path)| {
                Ok(Artifact {
                    id: index as u64 + 1,
                    media: PreviewMedia::load(path, &profile)?,
                    saved_path: None,
                    arrived: Instant::now(),
                    exit: None,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let next_id = artifacts.len() as u64 + 1;
        Ok(Self {
            launch,
            artifacts,
            placement,
            collapsed: false,
            transition: None,
            hovered: None,
            stack_hovered: false,
            copied: None,
            saved_feedback: None,
            saving: None,
            rejected: None,
            status: String::new(),
            next_id,
            reduced_motion: crate::theme::reduced_motion(),
        })
    }

    fn add(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        match PreviewMedia::load(path, &self.launch.profile) {
            Ok(media) => {
                self.artifacts.push(Artifact {
                    id: self.next_id,
                    media,
                    saved_path: None,
                    arrived: Instant::now(),
                    exit: None,
                });
                self.next_id += 1;
                self.status.clear();
                window.refresh();
                cx.notify();
            }
            Err(error) => {
                self.status = error.to_string();
                cx.notify();
            }
        }
    }

    fn glass_button(
        id: impl Into<ElementId>,
        label: &'static str,
        signal: bool,
        t: Theme,
    ) -> Stateful<Div> {
        let (icon, label) = button_content(label);
        div()
            .id(id)
            .h(px(28.))
            .min_w(px(28.))
            .px(px(7.))
            .rounded(px(6.))
            .border_1()
            .border_color(t.glass_border)
            .bg(t.glass)
            .text_color(if signal { t.signal } else { t.glass_text })
            .font_weight(FontWeight::SEMIBOLD)
            .flex()
            .items_center()
            .justify_center()
            .gap(px(5.))
            .cursor_pointer()
            .hover(move |style| {
                style
                    .bg(if signal { t.signal } else { rgba(0x2f3036f5) })
                    .text_color(t.glass_text)
            })
            .active(|style| style.opacity(0.8))
            .when_some(icon, |button, icon| {
                button.child(line_icon(icon, if signal { "#ef4650" } else { "#f6f6f8" }))
            })
            .when(!label.is_empty(), |button| button.child(label))
    }

    fn action_button(
        id: impl Into<ElementId>,
        label: &'static str,
        primary: bool,
        t: Theme,
    ) -> Stateful<Div> {
        let (icon, label) = button_content(label);
        div()
            .id(id)
            .w(px(140.))
            .h(px(32.))
            .rounded(px(6.))
            .border_1()
            .border_color(if primary {
                rgba(0x00000000)
            } else {
                rgba(0xffffff36)
            })
            .bg(if primary { t.accent } else { t.glass })
            .text_color(if primary { rgb(0x17140b) } else { t.glass_text })
            .font_weight(FontWeight::SEMIBOLD)
            .flex()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .cursor_pointer()
            .hover(|style| style.opacity(0.9))
            .active(|style| style.opacity(0.75))
            .when_some(icon, |button, icon| {
                button.child(line_icon(icon, if primary { "#17140b" } else { "#f6f6f8" }))
            })
            .child(label)
    }

    fn dismiss(&mut self, id: u64, delay_ms: u64, cx: &mut Context<Self>) {
        if let Some(artifact) = self.artifacts.iter_mut().find(|artifact| artifact.id == id)
            && artifact.exit.is_none()
        {
            artifact.exit = Some(Exit {
                kind: ExitKind::Dismiss,
                started: Instant::now(),
                delay_ms,
                dissolve: None,
            });
            cx.notify();
        }
    }

    fn delete(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(artifact) = self.artifacts.iter_mut().find(|artifact| artifact.id == id) else {
            return;
        };
        if artifact.exit.is_some() {
            return;
        }
        if let Some(saved) = artifact.saved_path.as_ref()
            && let Err(error) = fs::remove_file(saved)
        {
            self.status = format!("Delete failed: {error}");
            cx.notify();
            return;
        }
        match image::open(artifact.media.display.as_ref()) {
            Ok(image) => {
                let left_origin = if artifact.saved_path.is_some() {
                    57.5
                } else {
                    22.5
                };
                let origin_x = if self.placement.side == Side::Right {
                    CARD_WIDTH - left_origin
                } else {
                    left_origin
                };
                artifact.exit = Some(Exit {
                    kind: ExitKind::Delete,
                    started: Instant::now(),
                    delay_ms: 0,
                    dissolve: (!self.reduced_motion).then(|| {
                        Dissolve::new_from(
                            &image.to_rgba8(),
                            artifact.id as u32 + 83,
                            origin_x,
                            22.5,
                        )
                    }),
                });
            }
            Err(error) => self.status = format!("Delete failed: {error}"),
        }
        cx.notify();
    }

    fn copy(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(artifact) = self.artifacts.iter().find(|artifact| artifact.id == id) else {
            return;
        };
        match artifact.media.clipboard_content() {
            Ok(media::ClipboardContent::Image(image)) => {
                cx.write_to_clipboard(ClipboardItem::new_image(&image));
                self.copied = Some(id);
                self.status.clear();
            }
            Ok(media::ClipboardContent::File(path)) => match media::copy_file_to_clipboard(&path) {
                Ok(()) => {
                    self.copied = Some(id);
                    self.status.clear();
                }
                Err(error) => self.status = format!("Copy failed: {error}"),
            },
            Err(error) => self.status = format!("Copy failed: {error}"),
        }
        cx.notify();
    }

    fn save(&mut self, id: u64, cx: &mut Context<Self>) {
        if self.saving.is_some() {
            return;
        }
        let Some(artifact) = self.artifacts.iter().find(|artifact| artifact.id == id) else {
            return;
        };
        let media = artifact.media.clone();
        let profile = self.launch.profile.clone();
        self.saving = Some(id);
        let task = cx.background_executor().spawn(async move {
            let settings = settings::load(&profile)?;
            let path = media::save(&media, &settings)?;
            let history_error =
                crate::preferences::history::link_saved(&profile, &media.source, &path)
                    .err()
                    .map(|e| e.to_string());
            anyhow::Ok((path, history_error))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |s, cx| {
                s.saving = None;
                match result {
                    Ok((path, history_error)) => {
                        if let Some(artifact) =
                            s.artifacts.iter_mut().find(|artifact| artifact.id == id)
                        {
                            artifact.saved_path = Some(path);
                        }
                        s.saved_feedback = Some((id, Instant::now()));
                        s.status = history_error
                            .map(|e| format!("File saved; history link failed: {e}"))
                            .unwrap_or_default();
                    }
                    Err(error) => s.status = format!("Save failed: {error}"),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn reveal(&mut self, id: u64, cx: &mut Context<Self>) {
        let result = self
            .artifacts
            .iter()
            .find(|artifact| artifact.id == id)
            .and_then(|artifact| artifact.saved_path.as_deref())
            .ok_or_else(|| anyhow::anyhow!("Save this capture before showing it in its folder"))
            .and_then(media::reveal);
        if let Err(error) = result {
            self.status = error.to_string();
        }
        cx.notify();
    }

    fn open_editor(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(artifact) = self.artifacts.iter().find(|artifact| artifact.id == id) else {
            return;
        };
        let mut launch = self.launch.clone();
        launch.path = Some(artifact.media.source.clone());
        let view = if artifact.media.kind == MediaKind::Image {
            "screenshot-editor"
        } else {
            "recording-editor"
        };
        if let Err(error) = crate::open_view(view, launch, cx) {
            self.status = format!("Editor failed: {error}");
            cx.notify();
        }
    }

    fn finish_animations(&mut self) {
        self.artifacts.retain(|artifact| {
            let Some(exit) = &artifact.exit else {
                return true;
            };
            let elapsed = exit.started.elapsed().as_millis() as u64;
            let duration = match exit.kind {
                ExitKind::Dismiss => 1_030,
                ExitKind::Delete if exit.dissolve.is_some() => 2_900,
                ExitKind::Delete => 680,
            };
            elapsed < exit.delay_ms + duration
        });
        if self
            .saved_feedback
            .is_some_and(|(_, started)| started.elapsed().as_secs_f32() > 1.8)
        {
            self.saved_feedback = None;
        }
        if self
            .rejected
            .is_some_and(|(_, started)| started.elapsed().as_secs_f32() > 0.42)
        {
            self.rejected = None;
        }
    }

    fn stack_settle_index(&self, index: usize) -> usize {
        let visual_index =
            layout::expanded_visual_index(index, self.artifacts.len(), self.placement.anchor);
        let ready_before = match self.placement.anchor {
            Anchor::Top => self.artifacts[index + 1..]
                .iter()
                .filter(|artifact| exit_motion_ready(artifact))
                .count(),
            Anchor::Bottom => self.artifacts[..index]
                .iter()
                .filter(|artifact| exit_motion_ready(artifact))
                .count(),
        };
        visual_index - ready_before
    }

    fn render_card(
        &self,
        index: usize,
        frame_height: f32,
        collapse: f32,
        t: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let artifact = &self.artifacts[index];
        let id = artifact.id;
        let depth = self.artifacts.len() - 1 - index;
        let settled_index = self.stack_settle_index(index);
        let expanded_top = layout::expanded_card_top(
            settled_index,
            self.artifacts.len()
                - self
                    .artifacts
                    .iter()
                    .filter(|a| exit_motion_ready(a))
                    .count(),
            frame_height,
            self.placement.anchor,
        );
        let collapsed_top = layout::collapsed_card_top(
            depth,
            frame_height,
            self.placement.anchor,
            self.stack_hovered,
        );
        let mut top = layout::mix(expanded_top, collapsed_top, collapse);
        let mut left = 28. - layout::pose_depth(depth) * 0.8 * collapse;
        let mut opacity = (1. - layout::pose_depth(depth) * 0.08 * collapse).max(0.22);
        let mut display_dissolve = None;
        if let Some(exit) = &artifact.exit {
            let elapsed = exit.started.elapsed().as_millis() as i64 - exit.delay_ms as i64;
            if elapsed >= 0 {
                match exit.kind {
                    ExitKind::Dismiss => {
                        let progress = (elapsed as f32 / 450.).clamp(0., 1.);
                        let direction = if self.placement.side == Side::Right {
                            1.
                        } else {
                            -1.
                        };
                        left += direction * 118. * (1. - (1. - progress).powi(3));
                        opacity *= 1. - progress;
                    }
                    ExitKind::Delete => {
                        if let Some(effect) = &exit.dissolve {
                            display_dissolve =
                                Some(media::render_bgra(effect.frame(elapsed as f32)));
                        } else {
                            let progress = (elapsed as f32 / 680.).clamp(0., 1.);
                            opacity *= 1. - progress;
                        }
                    }
                }
            }
        } else if !self.reduced_motion {
            let progress = (artifact.arrived.elapsed().as_secs_f32() / 0.52).clamp(0., 1.);
            let eased = 1. - (1. - progress).powi(3);
            top += 24. * (1. - eased);
            opacity *= eased;
        }
        if let Some((rejected, started)) = self.rejected
            && rejected == id
        {
            left += layout::rejection_x(started.elapsed().as_secs_f32());
        }

        if let Some(dissolve) = display_dissolve {
            return div()
                .absolute()
                .left(px(left - PAD as f32))
                .top(px(top - PAD as f32))
                .w(px((WIDTH + 2 * PAD) as f32))
                .h(px((HEIGHT + 2 * PAD) as f32))
                .child(img(dissolve).size_full())
                .into_any_element();
        }

        let is_hovered = self.hovered == Some(id) && collapse < 0.01 && artifact.exit.is_none();
        let image = if is_hovered
            || artifact
                .exit
                .as_ref()
                .is_some_and(|exit| exit.kind == ExitKind::Dismiss)
        {
            img(artifact.media.hovered.clone())
        } else {
            img(artifact.media.display.clone())
        };
        let saved = artifact.saved_path.is_some();
        let copied = self.copied == Some(id);
        let saved_recently = self
            .saved_feedback
            .is_some_and(|(saved_id, _)| saved_id == id);
        let drag = DraggedCapture {
            id,
            source: artifact
                .saved_path
                .clone()
                .unwrap_or_else(|| artifact.media.source.clone()),
            poster: Arc::new(artifact.media.display.to_path_buf()),
        };

        let mut card = div()
            .id(("preview", id as usize))
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(CARD_WIDTH))
            .h(px(CARD_HEIGHT))
            .rounded(px(12.))
            .overflow_hidden()
            .border_1()
            .border_color(t.glass_border)
            .shadow_lg()
            .opacity(opacity)
            .cursor_pointer()
            .on_hover(cx.listener(move |preview, hovered: &bool, _, cx| {
                preview.hovered = hovered.then_some(id);
                if preview.collapsed {
                    preview.stack_hovered = *hovered;
                }
                cx.notify();
            }))
            .on_click(cx.listener(|preview, _, _, cx| {
                if preview.collapsed {
                    preview.collapsed = false;
                    preview.transition = Some(Instant::now());
                    cx.notify();
                }
            }))
            .child(image.size_full().object_fit(ObjectFit::Cover));

        if artifact.media.kind == MediaKind::Video && !is_hovered {
            card = card.child(
                div()
                    .absolute()
                    .left(px(126.))
                    .top(px(64.))
                    .w(px(34.))
                    .h(px(34.))
                    .rounded_full()
                    .bg(rgba(0x0f0f12d9))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(16.))
                    .child("▶"),
            );
        }

        if collapse < 0.01 && artifact.exit.is_none() {
            card = card.on_drag(drag, |value, _, _, cx| {
                if native_drag::supported() {
                    let handle = cx.global::<PreviewRegistry>().0;
                    let id = value.id;
                    native_drag::start_native_file_drag(
                        handle,
                        vec![value.source.clone()],
                        value.poster.as_ref().clone(),
                        cx,
                        move |result, cx| {
                            let _ = handle.update(cx, |preview, _, cx| {
                                match result {
                                    Ok(native_drag::NativeDragOutcome::Dropped) => {
                                        preview.status.clear()
                                    }
                                    Ok(native_drag::NativeDragOutcome::Cancelled) => {
                                        preview.rejected = Some((id, Instant::now()));
                                    }
                                    Err(error) => {
                                        preview.rejected = Some((id, Instant::now()));
                                        preview.status =
                                            format!("Could not drag capture: {error:#}");
                                    }
                                }
                                cx.notify();
                            });
                        },
                    );
                }
                cx.new(|_| DragGhost(value.clone()))
            });
            if is_hovered {
                let top_actions = div()
                    .absolute()
                    .top(px(8.))
                    .when(self.placement.side == Side::Left, |div| div.left(px(8.)))
                    .when(self.placement.side == Side::Right, |div| div.right(px(8.)))
                    .flex()
                    .gap(px(6.))
                    .when(saved, |div| {
                        div.child(
                            Self::glass_button(("close", id as usize), "×", false, t).on_click(
                                cx.listener(move |preview, _, _, cx| {
                                    cx.stop_propagation();
                                    preview.dismiss(id, 0, cx);
                                }),
                            ),
                        )
                    })
                    .child(
                        Self::glass_button(("delete", id as usize), "⌫", true, t).on_click(
                            cx.listener(move |_, _, window, cx| {
                                cx.stop_propagation();
                                let answer = window.prompt(
                                    PromptLevel::Critical,
                                    "Delete capture?",
                                    Some("The saved copy will be deleted. Capture History is kept for recovery."),
                                    &[
                                        PromptButton::Cancel("Cancel".into()),
                                        PromptButton::Ok("Delete".into()),
                                    ],
                                    cx,
                                );
                                cx.spawn_in(window, async move |this, cx| {
                                    if matches!(answer.await, Ok(1)) {
                                        let _ = this.update(cx, |preview, cx| preview.delete(id, cx));
                                    }
                                })
                                .detach();
                            }),
                        ),
                    );
                let edit = Self::glass_button(("edit", id as usize), "✎", false, t).on_click(
                    cx.listener(move |preview, _, _, cx| {
                        cx.stop_propagation();
                        preview.open_editor(id, cx);
                    }),
                );
                let actions = div()
                    .absolute()
                    .left(px(72.))
                    .top(px(if copied { 64. } else { 46. }))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .when(!copied, |div| {
                        div.child(
                            Self::action_button(("copy", id as usize), "▣  Copy", false, t)
                                .on_click(cx.listener(move |preview, _, _, cx| {
                                    cx.stop_propagation();
                                    preview.copy(id, cx);
                                })),
                        )
                    })
                    .child(
                        Self::action_button(
                            ("save", id as usize),
                            if self.saving == Some(id) {
                                "Saving…"
                            } else if saved_recently {
                                "✓  Saved"
                            } else if saved {
                                "□  Show in Folder"
                            } else {
                                "⇩  Save file"
                            },
                            true,
                            t,
                        )
                        .on_click(cx.listener(move |preview, _, _, cx| {
                            cx.stop_propagation();
                            if saved {
                                preview.reveal(id, cx);
                            } else {
                                preview.save(id, cx);
                            }
                        })),
                    );
                card = card
                    .child(top_actions)
                    .child(
                        div()
                            .absolute()
                            .top(px(8.))
                            .when(self.placement.side == Side::Left, |div| div.right(px(8.)))
                            .when(self.placement.side == Side::Right, |div| div.left(px(8.)))
                            .child(edit),
                    )
                    .child(actions);
            }
            let size = format_size(artifact.media.bytes);
            card = card.child(
                div()
                    .absolute()
                    .left(px(8.))
                    .right(px(8.))
                    .bottom(px(8.))
                    .flex()
                    .justify_between()
                    .text_size(px(10.))
                    .text_color(t.glass_muted)
                    .child(format!(
                        "{} × {} · {size}",
                        artifact.media.width, artifact.media.height
                    ))
                    .when(copied, |row| {
                        row.child(div().text_color(t.positive).child("✓ Copied to clipboard"))
                    }),
            );
        }
        card.into_any_element()
    }
}

impl Render for Preview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.reduced_motion = crate::theme::reduced_motion();
        self.finish_animations();
        let frame_height = f32::from(window.viewport_size().height);
        let transition_active = self
            .transition
            .is_some_and(|started| started.elapsed().as_secs_f32() < 0.52);
        let collapse = self
            .transition
            .map_or(if self.collapsed { 1. } else { 0. }, |started| {
                layout::collapse_progress(
                    started.elapsed().as_secs_f32(),
                    self.collapsed,
                    self.reduced_motion,
                )
            });
        if !transition_active {
            self.transition = None;
        }
        let animating = transition_active
            || self.artifacts.iter().any(|artifact| {
                artifact.exit.is_some() || artifact.arrived.elapsed().as_secs_f32() < 0.52
            })
            || self.saved_feedback.is_some()
            || self.rejected.is_some();
        if animating {
            window.request_animation_frame();
        }
        if self.artifacts.is_empty() && !self.launch.mock {
            window.remove_window();
        }
        let t = Theme::new(false);
        let mut body = div()
            .relative()
            .size_full()
            .font_family(font())
            .text_size(px(12.))
            .text_color(t.glass_text)
            .when(self.launch.mock, |div| div.bg(rgb(0x252b38)))
            .on_drop(cx.listener(|preview, dragged: &DraggedCapture, _, cx| {
                preview.rejected = Some((dragged.id, Instant::now()));
                preview.status = "Drop refused — the preview stays in Captures".into();
                cx.notify();
            }));

        for index in 0..self.artifacts.len() {
            body = body.child(self.render_card(index, frame_height, collapse, t, cx));
        }

        if !self.artifacts.is_empty() && collapse < 0.99 {
            let toolbar_top = match self.placement.anchor {
                Anchor::Top => 16.,
                Anchor::Bottom => {
                    frame_height - layout::expanded_height(self.artifacts.len(), frame_height) + 12.
                }
            };
            body = body.child(
                div()
                    .absolute()
                    .top(px(toolbar_top))
                    .when(self.placement.side == Side::Left, |div| div.left(px(28.)))
                    .when(self.placement.side == Side::Right, |div| div.right(px(28.)))
                    .flex()
                    .gap(px(8.))
                    .child(
                        Self::glass_button("collapse", "⌄  Show less", false, t).on_click(
                            cx.listener(|preview, _, _, cx| {
                                preview.collapsed = true;
                                preview.hovered = None;
                                preview.transition = Some(Instant::now());
                                cx.notify();
                            }),
                        ),
                    )
                    .when(self.artifacts.len() > 1, |div| {
                        div.child(
                            Self::glass_button("clear", "×  Clear all", false, t).on_click(
                                cx.listener(|preview, _, _, cx| {
                                    let ids = preview
                                        .artifacts
                                        .iter()
                                        .map(|artifact| artifact.id)
                                        .collect::<Vec<_>>();
                                    for (index, id) in ids.into_iter().rev().enumerate() {
                                        preview.dismiss(id, index as u64 * 55, cx);
                                    }
                                }),
                            ),
                        )
                    }),
            );
        }
        if self.collapsed && !self.artifacts.is_empty() {
            body = body.child(
                div()
                    .id("collapsed-hit-target")
                    .absolute()
                    .left(px(28.))
                    .top(px(match self.placement.anchor {
                        Anchor::Top => 30.,
                        Anchor::Bottom => frame_height - 230.,
                    }))
                    .w(px(CARD_WIDTH))
                    .h(px(210.))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
                    .on_hover(cx.listener(|preview, hovered: &bool, _, cx| {
                        preview.stack_hovered = *hovered;
                        cx.notify();
                    }))
                    .on_click(cx.listener(|preview, _, _, cx| {
                        preview.collapsed = false;
                        preview.transition = Some(Instant::now());
                        cx.notify();
                    })),
            );
        }
        if !self.status.is_empty() {
            body = body.child(
                div()
                    .absolute()
                    .left(px(36.))
                    .right(px(36.))
                    .bottom(px(10.))
                    .px(px(8.))
                    .py(px(5.))
                    .rounded(px(5.))
                    .bg(t.glass)
                    .text_color(t.glass_text)
                    .text_size(px(10.))
                    .child(self.status.clone()),
            );
        }
        body.into_any_element()
    }
}

fn exit_motion_ready(artifact: &Artifact) -> bool {
    artifact.exit.as_ref().is_some_and(|exit| {
        let threshold = exit.delay_ms
            + match exit.kind {
                ExitKind::Dismiss => 450,
                ExitKind::Delete if exit.dissolve.is_some() => 1_800,
                ExitKind::Delete => 0,
            };
        exit.started.elapsed().as_millis() as u64 >= threshold
    })
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024. * 1024.))
    }
}
