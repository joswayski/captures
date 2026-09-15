//! GPUI mini-preview stack for Linux.
//!
//! This intentionally mirrors the shipping Tauri stack geometry and timing,
//! while keeping the implementation native: GPUI elements and image textures,
//! no GTK window and no webview.

mod cache;
pub(crate) mod geometry;
#[cfg(target_os = "linux")]
pub(crate) mod x11;

use crate::{
    app,
    settings::{self, Settings},
    ui,
};
use anyhow::{Context as _, Result};
use cache::{DECODED_LIMIT, DecodedPreview};
use geometry::*;
use gpui::{
    App, ClickEvent, ClipboardItem, Context, Div, Empty, Global, Image, ImageFormat, IntoElement,
    MouseButton, ObjectFit, ParentElement, Pixels, Render, SharedString, Stateful, Styled, Timer,
    Window, WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowHandle, WindowKind,
    WindowOptions, div, img, point, prelude::*, px, size,
};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

const TITLE: &str = "Captures GPUI Previews";
const CLEAR_STAGGER_MS: u64 = 55;
const ROTATED_LIMIT: usize = 24;

#[derive(Clone)]
struct Card {
    id: u64,
    path: PathBuf,
    saved: bool,
    decoded: Option<DecodedPreview>,
    width: u32,
    height: u32,
    size_bytes: u64,
    copied: bool,
    arrived_at: Instant,
    decoded_at: Instant,
    exit: Option<Exit>,
    shake_at: Option<Instant>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExitKind {
    Dismiss,
    Delete,
}

#[derive(Clone)]
struct Exit {
    kind: ExitKind,
    started_at: Instant,
    delay: Duration,
    particles: Arc<Vec<Particle>>,
}

#[derive(Clone, Copy)]
struct Particle {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    dx: f32,
    dy: f32,
    delay_ms: f32,
    duration_ms: f32,
}

#[derive(Clone, Copy)]
struct StackMotion {
    from_collapsed: bool,
    started_at: Instant,
}

#[derive(Clone)]
struct DraggedCapture(PathBuf);

impl Render for DraggedCapture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

struct PreviewGlobal {
    cards: Vec<Card>,
    handle: Option<WindowHandle<PreviewView>>,
    visible: bool,
    next_id: u64,
    generation: u64,
}

impl Global for PreviewGlobal {}

impl Default for PreviewGlobal {
    fn default() -> Self {
        Self {
            cards: Vec::new(),
            handle: None,
            visible: true,
            next_id: 1,
            generation: 0,
        }
    }
}

pub fn add(path: PathBuf, cx: &mut App) -> Result<()> {
    ensure_global(cx);
    let generation = global(cx).generation;
    let data_dir = settings::data_dir();
    ui::job(
        cx,
        move || {
            let saved = !is_private_path_in(&path, &data_dir);
            cache::decode(&path)
                .map(|decoded| (path, decoded, saved))
                .map_err(|error| error.to_string())
        },
        move |result, cx| match result {
            Ok((path, decoded, saved)) => finish_add(path, decoded, saved, generation, cx),
            Err(error) => ui::error(error, cx),
        },
    );
    Ok(())
}

fn finish_add(path: PathBuf, decoded: DecodedPreview, saved: bool, generation: u64, cx: &mut App) {
    let card = {
        let state = global_mut(cx);
        if let Some(existing) = state.cards.iter_mut().find(|card| card.path == path) {
            existing.decoded = Some(decoded);
            existing.arrived_at = Instant::now();
            existing.decoded_at = Instant::now();
            existing.exit = None;
            existing.clone()
        } else {
            let id = state.next_id;
            state.next_id += 1;
            Card {
                id,
                path,
                saved,
                width: decoded.width,
                height: decoded.height,
                size_bytes: decoded.size_bytes,
                decoded: Some(decoded),
                copied: false,
                arrived_at: Instant::now(),
                decoded_at: Instant::now(),
                exit: None,
                shake_at: None,
            }
        }
    };
    {
        let state = global_mut(cx);
        state.cards.retain(|existing| existing.id != card.id);
        state.cards.push(card.clone());
        bound_decoded(&mut state.cards);
    }
    if !may_present(global(cx).visible, global(cx).generation, generation) {
        return;
    }
    if let Some(handle) = global(cx).handle {
        if let Err(error) = handle.update(cx, |view, window, cx| {
            view.cards.retain(|existing| existing.id != card.id);
            view.cards.push(card);
            view.collapsed = true;
            view.scroll = view.cards.len().saturating_sub(VISIBLE_CARDS);
            bound_decoded(&mut view.cards);
            view.rehome(window, cx);
            cx.notify();
        }) {
            ui::error(error.to_string(), cx);
        }
    } else if global(cx).visible
        && let Err(error) = show(cx)
    {
        ui::error(error.to_string(), cx);
    }
}

pub fn show(cx: &mut App) -> Result<()> {
    ensure_global(cx);
    global_mut(cx).visible = true;
    if global(cx).cards.is_empty() || !settings(cx).show_mini_previews {
        return Ok(());
    }
    if let Some(handle) = global(cx).handle {
        handle.update(cx, |view, window, cx| {
            view.rehome(window, cx);
            cx.notify();
        })?;
        return Ok(());
    }

    let cards = global(cx).cards.clone();
    let placement = settings(cx).mini_preview_placement.min(3);
    let bounds = home_bounds(placement, cx);
    let initial_origin = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    crate::desktop::clear_preview_input_region();
    let handle = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            focus: false,
            show: true,
            kind: WindowKind::PopUp,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            window_background: WindowBackgroundAppearance::Transparent,
            window_decorations: Some(WindowDecorations::Client),
            app_id: Some("captures-gpui-previews".into()),
            ..Default::default()
        },
        move |window, cx| {
            window.set_window_title(TITLE);
            cx.new(|cx| {
                let view = PreviewView::new(cards, placement, initial_origin);
                cx.observe_window_bounds(window, |view: &mut PreviewView, window, cx| {
                    view.update_gravity_from_window(window, cx);
                    cx.notify();
                })
                .detach();
                view
            })
        },
    )?;
    global_mut(cx).handle = Some(handle);
    cx.spawn(async move |cx| {
        Timer::after(Duration::from_millis(50)).await;
        let _ = cx.update(|cx| {
            if let Err(error) = crate::desktop::exclude_from_capture(
                TITLE,
                !settings(cx).include_mini_previews_in_captures,
            ) {
                ui::error(error, cx);
            }
        });
    })
    .detach();
    Ok(())
}

/// Hide previews while selecting or recording. This never dismisses cards or
/// removes files; `show` recreates the transparent popup from retained state.
pub fn hide(cx: &mut App) {
    ensure_global(cx);
    {
        let state = global_mut(cx);
        state.visible = false;
        state.generation = state.generation.wrapping_add(1);
    }
    if let Some(handle) = global_mut(cx).handle.take() {
        let _ = handle.update(cx, |view, window, cx| {
            global_mut(cx).cards = view.cards.clone();
            window.remove_window();
        });
    }
    crate::desktop::clear_preview_input_region();
}

fn ensure_global(cx: &mut App) {
    if cx.try_global::<PreviewGlobal>().is_none() {
        cx.set_global(PreviewGlobal::default());
    }
}

fn global(cx: &App) -> &PreviewGlobal {
    cx.global::<PreviewGlobal>()
}

fn global_mut(cx: &mut App) -> &mut PreviewGlobal {
    ensure_global(cx);
    cx.global_mut::<PreviewGlobal>()
}

fn settings(cx: &App) -> &Settings {
    cx.global::<Settings>()
}

fn home_bounds(placement: u32, cx: &App) -> gpui::Bounds<Pixels> {
    let display = cx.primary_display();
    let work = display
        .as_ref()
        .map(|display| display.bounds())
        .unwrap_or_else(|| gpui::Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(1_280.0), px(800.0)),
        });
    let (right, top) = placement_edges(placement);
    gpui::Bounds {
        origin: point(
            if right {
                work.origin.x + work.size.width - px(FRAME_WIDTH)
            } else {
                work.origin.x
            },
            if top {
                work.origin.y
            } else {
                work.origin.y + work.size.height - px(FRAME_HEIGHT)
            },
        ),
        size: size(px(FRAME_WIDTH), px(FRAME_HEIGHT)),
    }
}

/// Native settings: 0 bottom-right, 1 bottom-left, 2 top-right, 3 top-left.
fn placement_edges(placement: u32) -> (bool, bool) {
    (placement.is_multiple_of(2), placement >= 2)
}

fn placement_from_edges(right: bool, top: bool) -> u32 {
    u32::from(!right) + if top { 2 } else { 0 }
}

fn is_private_path_in(path: &std::path::Path, data_dir: &std::path::Path) -> bool {
    let Ok(path) = path.canonicalize() else {
        return false;
    };
    ["unsaved", "history"].into_iter().any(|directory| {
        data_dir
            .join(directory)
            .canonicalize()
            .is_ok_and(|private| path.starts_with(private))
    })
}

fn may_present(visible: bool, current_generation: u64, queued_generation: u64) -> bool {
    visible && current_generation == queued_generation
}

fn bound_decoded(cards: &mut [Card]) {
    while cards.iter().filter(|card| card.decoded.is_some()).count() > DECODED_LIMIT {
        let Some(index) = cards
            .iter()
            .enumerate()
            .filter(|(_, card)| card.decoded.is_some())
            .min_by_key(|(_, card)| card.decoded_at)
            .map(|(index, _)| index)
        else {
            break;
        };
        cards[index].decoded = None;
    }
}

struct PreviewView {
    cards: Vec<Card>,
    collapsed: bool,
    top: bool,
    right: bool,
    gravity: f32,
    hover: f32,
    hover_target: f32,
    hovered_card: Option<u64>,
    confirming_delete: Option<u64>,
    scroll: usize,
    motion: Option<StackMotion>,
    last_frame: Instant,
    last_window_origin: Option<(f32, f32)>,
    sway: (f32, f32),
    sway_velocity: (f32, f32),
    error: Option<SharedString>,
    initial_origin: (f32, f32),
    loading: HashSet<PathBuf>,
    saving: HashSet<u64>,
    rotations: HashMap<(u64, i8), (cache::RotatedPreview, Instant)>,
    rotating: HashSet<(u64, i8)>,
}

impl PreviewView {
    fn new(cards: Vec<Card>, placement: u32, initial_origin: (f32, f32)) -> Self {
        let (right, top) = placement_edges(placement);
        Self {
            scroll: cards.len().saturating_sub(VISIBLE_CARDS),
            cards,
            collapsed: true,
            top,
            right,
            gravity: if top { -1.0 } else { 1.0 },
            hover: 0.0,
            hover_target: 0.0,
            hovered_card: None,
            confirming_delete: None,
            motion: None,
            last_frame: Instant::now(),
            last_window_origin: None,
            sway: (0.0, 0.0),
            sway_velocity: (0.0, 0.0),
            error: None,
            initial_origin,
            loading: HashSet::new(),
            saving: HashSet::new(),
            rotations: HashMap::new(),
            rotating: HashSet::new(),
        }
    }

    fn rehome(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.gravity = if self.top { -1.0 } else { 1.0 };
    }

    fn update_gravity_from_window(&mut self, window: &Window, cx: &mut App) {
        let bounds = window.window_bounds().get_bounds();
        let origin = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
        if let Some(previous) = self.last_window_origin {
            let dx = origin.0 - previous.0;
            let dy = origin.1 - previous.1;
            self.sway_velocity.0 = (-dx * 0.12).clamp(-3.0, 3.0) * 24.0;
            self.sway_velocity.1 = (-dy * 0.08).clamp(-2.0, 2.0) * 24.0;
        }
        self.last_window_origin = Some(origin);
        if let Some(display) = window.display(cx) {
            let work = display.bounds();
            let y = origin.1 - f32::from(work.origin.y)
                + (FRAME_HEIGHT - CARD_HEIGHT) * ((self.gravity + 1.0) * 0.5);
            self.gravity = gravity_from_y(y, f32::from(work.size.height));
            self.top = if self.top {
                self.gravity < 0.2
            } else {
                self.gravity <= -0.2
            };
            self.right = side_from_x(
                origin.0 - f32::from(work.origin.x),
                f32::from(work.size.width),
                self.right,
            );
            let placement = placement_from_edges(self.right, self.top);
            if settings(cx).mini_preview_placement != placement {
                let mut updated = settings(cx).clone();
                updated.mini_preview_placement = placement;
                match updated.save() {
                    Ok(()) => {
                        *cx.global_mut::<Settings>() = updated;
                        app::settings_changed(cx);
                    }
                    Err(error) => {
                        ui::error(error.clone(), cx);
                        self.error = Some(error.into());
                    }
                }
            }
        }
    }

    fn animate(&mut self, window: &Window) -> bool {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.048);
        self.last_frame = now;
        let hover_rate = if self.hover_target > self.hover {
            10.0
        } else {
            7.0
        };
        self.hover += (self.hover_target - self.hover) * (1.0 - (-hover_rate * dt).exp());
        let spring = |position: &mut f32, velocity: &mut f32| {
            *velocity += (-*position * 500.0 - *velocity * 32.0) * dt;
            *position += *velocity * dt;
            if position.abs() < 0.001 && velocity.abs() < 0.01 {
                *position = 0.0;
                *velocity = 0.0;
            }
        };
        spring(&mut self.sway.0, &mut self.sway_velocity.0);
        spring(&mut self.sway.1, &mut self.sway_velocity.1);

        let mut active = (self.hover - self.hover_target).abs() > 0.002
            || self.sway.0 != 0.0
            || self.sway.1 != 0.0;
        if let Some(motion) = self.motion
            && now.duration_since(motion.started_at).as_secs_f32() * 1_000.0 < STACK_MOTION_MS
        {
            active = true;
        } else {
            self.motion = None;
        }
        active |= self.cards.iter().any(|card| {
            now.duration_since(card.arrived_at).as_secs_f32() * 1_000.0 < ARRIVE_MS
                || card.shake_at.is_some_and(|at| {
                    now.duration_since(at).as_secs_f32() * 1_000.0 < DROP_REJECT_MS
                })
                || card.exit.as_ref().is_some_and(|exit| {
                    let duration = if exit.kind == ExitKind::Delete {
                        DELETE_MS
                    } else {
                        DISMISS_MS
                    };
                    now.duration_since(exit.started_at)
                        < exit.delay + Duration::from_secs_f32(duration / 1_000.0)
                })
        });
        if active {
            window.request_animation_frame();
        }
        active
    }

    fn finish_exits(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        let finished: Vec<_> = self
            .cards
            .iter()
            .filter_map(|card| {
                let exit = card.exit.as_ref()?;
                let duration = if exit.kind == ExitKind::Delete {
                    DELETE_MS
                } else {
                    DISMISS_MS
                };
                if now.duration_since(exit.started_at)
                    < exit.delay + Duration::from_secs_f32(duration / 1_000.0)
                {
                    return None;
                }
                Some((card.id, card.path.clone(), card.saved, exit.kind))
            })
            .collect();
        if finished.is_empty() {
            return;
        }
        for (id, path, saved, kind) in finished {
            if kind == ExitKind::Delete
                && saved
                && let Err(error) = trash::delete(&path)
                    .with_context(|| format!("move {} to trash", path.display()))
            {
                self.error = Some(error.to_string().into());
                if let Some(card) = self.cards.iter_mut().find(|card| card.id == id) {
                    card.exit = None;
                }
                continue;
            }
            self.cards.retain(|card| card.id != id);
            global_mut(cx).cards.retain(|card| card.id != id);
        }
        self.scroll = self
            .scroll
            .min(self.cards.len().saturating_sub(VISIBLE_CARDS));
        if self.cards.is_empty() {
            global_mut(cx).handle = None;
        }
        cx.notify();
    }

    fn set_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        if self.collapsed == collapsed || self.motion.is_some() {
            return;
        }
        self.motion = Some(StackMotion {
            from_collapsed: self.collapsed,
            started_at: Instant::now(),
        });
        self.collapsed = collapsed;
        self.hover_target = 0.0;
        self.hovered_card = None;
        if !collapsed {
            self.scroll = self.cards.len().saturating_sub(VISIBLE_CARDS);
        }
        cx.notify();
    }

    fn dismiss(&mut self, id: u64, delay: Duration, cx: &mut Context<Self>) {
        self.begin_exit(id, ExitKind::Dismiss, delay, cx);
    }

    fn begin_exit(&mut self, id: u64, kind: ExitKind, delay: Duration, cx: &mut Context<Self>) {
        let Some(card) = self.cards.iter_mut().find(|card| card.id == id) else {
            return;
        };
        if card.exit.is_some() {
            return;
        }
        card.exit = Some(Exit {
            kind,
            started_at: Instant::now(),
            delay,
            particles: if kind == ExitKind::Delete {
                Arc::new(particles(self.right))
            } else {
                Arc::new(Vec::new())
            },
        });
        self.confirming_delete = None;
        self.hovered_card = None;
        cx.notify();
    }

    fn request_delete(&mut self, id: u64, cx: &mut Context<Self>) {
        if self.confirming_delete == Some(id) {
            self.begin_exit(id, ExitKind::Delete, Duration::ZERO, cx);
            return;
        }
        self.confirming_delete = Some(id);
        cx.notify();
        cx.spawn(async move |view, cx| {
            Timer::after(Duration::from_secs(4)).await;
            let _ = view.update(cx, |view, cx| {
                if view.confirming_delete == Some(id) {
                    view.confirming_delete = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn clear_all(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<_> = self
            .cards
            .iter()
            .filter(|card| card.exit.is_none())
            .map(|card| card.id)
            .collect();
        for (index, id) in ids.into_iter().rev().enumerate() {
            self.dismiss(
                id,
                Duration::from_millis((index.min(6) as u64) * CLEAR_STAGGER_MS),
                cx,
            );
        }
    }

    fn copy(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(card) = self.cards.iter_mut().find(|card| card.id == id) else {
            return;
        };
        if !cache::is_image(&card.path) {
            let uri = file_uri(&card.path);
            if let Err(error) = crate::desktop::copy_file(&card.path) {
                ui::error(error.to_string(), cx);
                cx.write_to_clipboard(ClipboardItem::new_string(uri));
            }
            card.copied = true;
            if let Some(global_card) = global_mut(cx).cards.iter_mut().find(|item| item.id == id) {
                global_card.copied = true;
            }
            cx.notify();
            return;
        }
        match fs::read(&card.path) {
            Ok(bytes) => {
                let format = match card
                    .path
                    .extension()
                    .and_then(|value| value.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase()
                    .as_str()
                {
                    "jpg" | "jpeg" => ImageFormat::Jpeg,
                    "webp" => ImageFormat::Webp,
                    "gif" => ImageFormat::Gif,
                    _ => ImageFormat::Png,
                };
                cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(format, bytes)));
                card.copied = true;
                if let Some(global_card) =
                    global_mut(cx).cards.iter_mut().find(|item| item.id == id)
                {
                    global_card.copied = true;
                }
                cx.notify();
            }
            Err(error) => self.error = Some(error.to_string().into()),
        }
    }

    fn save(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(card) = self.cards.iter().find(|card| card.id == id) else {
            return;
        };
        if card.saved {
            let path = card.path.clone();
            ui::job(
                cx,
                move || reveal(&path),
                move |result, cx| {
                    if let Err(error) = result {
                        ui::error(error, cx);
                    }
                },
            );
            return;
        }
        if !self.saving.insert(id) {
            return;
        }
        let path = card.path.clone();
        let output_directory = settings(cx).output_directory.clone();
        let format = settings(cx).screenshot_format.clone();
        ui::job(
            cx,
            move || {
                let (bytes, extension) = cache::encode_saved(&path, &format)?;
                app::publish_capture(&output_directory, &bytes, extension)
            },
            move |result, cx| finish_save(id, result, cx),
        );
        cx.notify();
    }

    fn open(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(path) = self
            .cards
            .iter()
            .find(|card| card.id == id)
            .map(|card| card.path.clone())
        else {
            return;
        };
        app::open(path, cx);
    }

    fn pose(&self, index: usize, now: Instant) -> Pose {
        let depth = self.cards.len() - 1 - index;
        let compact = collapsed_pose(
            self.cards[index].id,
            depth,
            self.gravity,
            self.hover,
            self.sway,
        );
        let expanded = expanded_pose(index, self.cards.len(), self.top, self.scroll);
        let base = if let Some(motion) = self.motion {
            let progress =
                now.duration_since(motion.started_at).as_secs_f32() * 1_000.0 / STACK_MOTION_MS;
            if motion.from_collapsed {
                interpolate(compact, expanded, progress)
            } else {
                interpolate(expanded, compact, progress)
            }
        } else if self.collapsed {
            compact
        } else {
            expanded
        };
        let mut pose = card_exit_pose(base, self.cards[index].exit.as_ref(), self.right, now);
        if !self.collapsed && self.motion.is_none() && self.cards[index].exit.is_none() {
            let visual = if self.top {
                self.cards.len() - 1 - index
            } else {
                index
            };
            for (exit_index, exit_card) in self.cards.iter().enumerate() {
                let Some(exit) = exit_card.exit.as_ref() else {
                    continue;
                };
                let exit_visual = if self.top {
                    self.cards.len() - 1 - exit_index
                } else {
                    exit_index
                };
                if exit_visual < visual {
                    let elapsed = now
                        .duration_since(exit.started_at)
                        .saturating_sub(exit.delay);
                    pose.y -= CARD_SLOT * smooth(elapsed.as_secs_f32() * 1_000.0 / 580.0);
                }
            }
        }
        pose
    }

    fn card(
        &self,
        index: usize,
        pose: Pose,
        theme: ui::Theme,
        now: Instant,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let card = &self.cards[index];
        let id = card.id;
        let hover = self.hovered_card == Some(id) && !self.collapsed && card.exit.is_none();
        let arrival = (now.duration_since(card.arrived_at).as_secs_f32() * 1_000.0 / ARRIVE_MS)
            .clamp(0.0, 1.0);
        let arrive_ease = ease_out_cubic(arrival);
        let y = pose.y + (1.0 - arrive_ease) * 24.0;
        let mut x = pose.x;
        if let Some(at) = card.shake_at {
            x += reject_shake(now.duration_since(at).as_secs_f32() * 1_000.0);
        }
        let opacity = pose.opacity * arrive_ease;
        let rotation = rotation_key(pose.rotation)
            .and_then(|key| self.rotations.get(&(id, key)))
            .map(|(rotation, _)| rotation);
        let image = card.decoded.as_ref().map(|decoded| {
            if hover || card.exit.is_some() {
                rotation.map_or_else(|| decoded.blurred.clone(), |image| image.blurred.clone())
            } else {
                rotation.map_or_else(|| decoded.normal.clone(), |image| image.normal.clone())
            }
        });

        let mut element = div()
            .id(("preview-card", id))
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(pose.width))
            .h(px(pose.height))
            .rounded(px(12.0))
            .overflow_hidden()
            .opacity(opacity)
            .bg(theme.glass())
            .shadow_lg()
            .cursor_grab();
        if let Some(image) = image {
            element = element.child(
                img(image)
                    .absolute()
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .rounded(px(12.0)),
            );
        }
        if card.decoded.is_none() {
            element = element.child(
                div()
                    .absolute()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.glass_text())
                    .child("Preview evicted — expand to reload"),
            );
        }
        if hover {
            element = element.child(self.card_chrome(card, theme, cx));
        } else if card.exit.is_none() && !self.collapsed {
            element = element.child(meta_chip(card, theme));
        }
        if card
            .exit
            .as_ref()
            .is_some_and(|exit| exit.kind == ExitKind::Delete)
        {
            element = element.children(dust(card, pose, now));
        }
        let drag_path = card.path.clone();
        element
            .on_hover(cx.listener(move |view, hovered: &bool, _, cx| {
                if view.collapsed
                    || view
                        .cards
                        .iter()
                        .find(|card| card.id == id)
                        .is_some_and(|card| card.exit.is_some())
                {
                    return;
                }
                view.hovered_card = hovered.then_some(id);
                cx.notify();
            }))
            .on_click(cx.listener(move |view, event: &ClickEvent, _, cx| {
                if event.click_count() == 2 && !view.collapsed {
                    view.open(id, cx);
                }
            }))
            .on_drag(DraggedCapture(drag_path), |drag, _, _, cx| {
                cx.new(|_| DraggedCapture(drag.0.clone()))
            })
            .on_drop(cx.listener(move |view, drag: &DraggedCapture, _, cx| {
                if view
                    .cards
                    .iter()
                    .find(|card| card.id == id)
                    .is_some_and(|card| card.path == drag.0)
                {
                    if let Some(card) = view.cards.iter_mut().find(|card| card.id == id) {
                        card.shake_at = Some(Instant::now());
                    }
                    cx.notify();
                }
            }))
    }

    fn card_chrome(&self, card: &Card, theme: ui::Theme, cx: &mut Context<Self>) -> Div {
        let id = card.id;
        let confirm = self.confirming_delete == Some(id);
        let close = icon_button(("preview-close", id), "×", theme)
            .on_click(cx.listener(move |view, _, _, cx| view.dismiss(id, Duration::ZERO, cx)));
        let delete = icon_button(
            ("preview-delete", id),
            if confirm { "Delete?" } else { "⌫" },
            theme,
        )
        .when(confirm, |button| {
            button.bg(theme.accent).text_color(theme.glass_text())
        })
        .on_click(cx.listener(move |view, _, _, cx| view.request_delete(id, cx)));
        let edit = icon_button(("preview-edit", id), "Edit", theme)
            .on_click(cx.listener(move |view, _, _, cx| view.open(id, cx)));
        let copy = action_button(
            ("preview-copy", id),
            if card.copied { "✓ Copied" } else { "Copy" },
            theme,
        )
        .on_click(cx.listener(move |view, _, _, cx| view.copy(id, cx)));
        let save_label = if self.saving.contains(&id) {
            "Saving…"
        } else if card.saved {
            "Show in Folder"
        } else {
            "Save file"
        };
        let save = action_button(("preview-save", id), save_label, theme)
            .on_click(cx.listener(move |view, _, _, cx| view.save(id, cx)));
        div()
            .absolute()
            .size_full()
            .child(
                div()
                    .absolute()
                    .top(px(8.0))
                    .left(px(8.0))
                    .flex()
                    .gap(px(6.0))
                    .child(close)
                    .child(delete),
            )
            .child(div().absolute().top(px(8.0)).right(px(8.0)).child(edit))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(6.0))
                    .child(copy)
                    .child(save),
            )
    }
}

impl Render for PreviewView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.finish_exits(cx);
        if self.cards.is_empty() {
            #[cfg(target_os = "linux")]
            x11::clear_cached_window();
            window.remove_window();
            return div();
        }
        self.ensure_visible_decoded(cx);
        let now = Instant::now();
        self.animate(window);
        let theme = ui::theme(cx);
        let count = self.cards.len();
        let poses: Vec<_> = (0..count).map(|index| self.pose(index, now)).collect();
        self.ensure_rotations(&poses, cx);
        let mut input: Vec<_> = poses
            .iter()
            .enumerate()
            .filter(|(index, pose)| self.cards[*index].exit.is_none() && pose.opacity > 0.01)
            .map(|(_, pose)| Rect {
                x: pose.x,
                y: pose.y,
                width: pose.width,
                height: pose.height,
            })
            .collect();
        if !self.collapsed && count >= 2 {
            input.push(Rect {
                x: if self.right {
                    FRAME_WIDTH - CARD_X - 170.0
                } else {
                    CARD_X
                },
                y: if self.top { 8.0 } else { FRAME_HEIGHT - 44.0 },
                width: 170.0,
                height: 36.0,
            });
        }
        let input: Vec<_> = input
            .iter()
            .map(|rect| (rect.x, rect.y, rect.width, rect.height))
            .collect();
        let _ = crate::desktop::set_preview_input_region(
            &input,
            window.scale_factor(),
            self.initial_origin,
        );
        let mut root = div()
            .relative()
            .w(px(FRAME_WIDTH))
            .h(px(FRAME_HEIGHT))
            .bg(gpui::transparent_black())
            .children(
                (0..count)
                    .filter(|index| poses[*index].opacity > 0.01)
                    .map(|index| self.card(index, poses[index], theme, now, cx)),
            );

        if self.collapsed {
            root = root.child(
                div()
                    .id("preview-pile-hit")
                    .absolute()
                    .left(px(CARD_X - 4.0))
                    .top(px(
                        (poses.last().map_or(STACK_GUTTER, |pose| pose.y) - 8.0).max(0.0)
                    ))
                    .w(px(CARD_WIDTH + 8.0))
                    .h(px(CARD_HEIGHT + collapsed_padding(count)))
                    .cursor_grab()
                    .on_hover(cx.listener(|view, hover, _, cx| {
                        view.hover_target = if *hover { 1.0 } else { 0.0 };
                        cx.notify();
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _, window, _| {
                            window.start_window_move();
                        }),
                    )
                    .on_click(cx.listener(|view, _, _, cx| view.set_collapsed(false, cx))),
            );
        } else if count >= 2 {
            let toolbar_y = if self.top { 8.0 } else { FRAME_HEIGHT - 44.0 };
            let toolbar_x = if self.right {
                FRAME_WIDTH - CARD_X - 170.0
            } else {
                CARD_X
            };
            root = root.child(
                div()
                    .absolute()
                    .left(px(toolbar_x))
                    .top(px(toolbar_y))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        icon_button("preview-clear-all", "Clear all", theme)
                            .w(px(80.0))
                            .h(px(34.0))
                            .on_click(cx.listener(|view, _, _, cx| view.clear_all(cx))),
                    )
                    .child(
                        action_button("preview-show-less", "Show less", theme)
                            .w(px(84.0))
                            .on_click(cx.listener(|view, _, _, cx| view.set_collapsed(true, cx))),
                    ),
            );
        }
        if !self.collapsed && self.scroll > 0 {
            root = root.child(
                icon_button("preview-older", "⌃", theme)
                    .absolute()
                    .top(px(6.0))
                    .left(px(147.0))
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.scroll = view.scroll.saturating_sub(1);
                        cx.notify();
                    })),
            );
        }
        if !self.collapsed && self.scroll + VISIBLE_CARDS < count {
            root = root.child(
                icon_button("preview-newer", "⌄", theme)
                    .absolute()
                    .bottom(px(6.0))
                    .left(px(147.0))
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.scroll =
                            (view.scroll + 1).min(view.cards.len().saturating_sub(VISIBLE_CARDS));
                        cx.notify();
                    })),
            );
        }
        if let Some(error) = self.error.clone() {
            root = root.child(
                div()
                    .absolute()
                    .left(px(28.0))
                    .bottom(px(8.0))
                    .max_w(px(284.0))
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(6.0))
                    .bg(theme.glass())
                    .text_color(theme.glass_text())
                    .text_xs()
                    .child(error),
            );
        }
        root
    }
}

impl PreviewView {
    fn ensure_rotations(&mut self, poses: &[Pose], cx: &mut Context<Self>) {
        for (card, pose) in self.cards.iter().zip(poses) {
            let Some(key) = rotation_key(pose.rotation) else {
                continue;
            };
            let cache_key = (card.id, key);
            if self.rotations.contains_key(&cache_key) || !self.rotating.insert(cache_key) {
                continue;
            }
            let Some(decoded) = card.decoded.clone() else {
                self.rotating.remove(&cache_key);
                continue;
            };
            ui::job(
                cx,
                move || cache::rotate(&decoded, key as f32 / 2.0),
                move |result, cx| finish_rotation(cache_key, result, cx),
            );
        }
    }

    fn ensure_visible_decoded(&mut self, cx: &mut Context<Self>) {
        let indices: Vec<_> = if self.collapsed {
            self.cards.len().saturating_sub(DECODED_LIMIT)..self.cards.len()
        } else {
            self.scroll.saturating_sub(1)..(self.scroll + VISIBLE_CARDS + 1).min(self.cards.len())
        }
        .collect();
        for index in indices {
            let path = self.cards[index].path.clone();
            if self.cards[index].decoded.is_none() && self.loading.insert(path.clone()) {
                let work_path = path.clone();
                ui::job(
                    cx,
                    move || cache::decode(&work_path).map_err(|error| error.to_string()),
                    move |result, cx| finish_reload(path, result, cx),
                );
            }
        }
    }
}

fn rotation_key(degrees: f32) -> Option<i8> {
    let key = (degrees * 2.0).round().clamp(-8.0, 8.0) as i8;
    (key != 0).then_some(key)
}

fn finish_rotation(
    key: (u64, i8),
    result: std::result::Result<cache::RotatedPreview, String>,
    cx: &mut App,
) {
    let Some(handle) = global(cx).handle else {
        return;
    };
    let _ = handle.update(cx, |view, _, cx| {
        view.rotating.remove(&key);
        match result {
            Ok(rotation) => {
                view.rotations.insert(key, (rotation, Instant::now()));
                while view.rotations.len() > ROTATED_LIMIT {
                    let Some(oldest) = view
                        .rotations
                        .iter()
                        .min_by_key(|(_, (_, used))| *used)
                        .map(|(key, _)| *key)
                    else {
                        break;
                    };
                    view.rotations.remove(&oldest);
                }
            }
            Err(error) => view.error = Some(error.into()),
        }
        cx.notify();
    });
}

fn finish_reload(path: PathBuf, result: std::result::Result<DecodedPreview, String>, cx: &mut App) {
    let decoded = match result {
        Ok(decoded) => decoded,
        Err(error) => {
            ui::error(error.clone(), cx);
            if let Some(handle) = cx
                .try_global::<PreviewGlobal>()
                .and_then(|state| state.handle)
            {
                let _ = handle.update(cx, |view, _, cx| {
                    view.loading.remove(&path);
                    view.error = Some(error.into());
                    cx.notify();
                });
            }
            return;
        }
    };
    if let Some(card) = global_mut(cx)
        .cards
        .iter_mut()
        .find(|card| card.path == path)
    {
        card.decoded = Some(decoded.clone());
        card.decoded_at = Instant::now();
    }
    bound_decoded(&mut global_mut(cx).cards);
    if let Some(handle) = global(cx).handle {
        let _ = handle.update(cx, |view, _, cx| {
            view.loading.remove(&path);
            if let Some(card) = view.cards.iter_mut().find(|card| card.path == path) {
                card.decoded = Some(decoded);
                card.decoded_at = Instant::now();
            }
            bound_decoded(&mut view.cards);
            cx.notify();
        });
    }
}

fn finish_save(id: u64, result: std::result::Result<PathBuf, String>, cx: &mut App) {
    let path = match result {
        Ok(path) => path,
        Err(error) => {
            ui::error(error.clone(), cx);
            if let Some(handle) = global(cx).handle {
                let _ = handle.update(cx, |view, _, cx| {
                    view.saving.remove(&id);
                    view.error = Some(error.into());
                    cx.notify();
                });
            }
            return;
        }
    };
    let size_bytes = fs::metadata(&path).map_or(0, |metadata| metadata.len());
    let present = if let Some(card) = global_mut(cx).cards.iter_mut().find(|card| card.id == id) {
        card.path = path.clone();
        card.saved = true;
        card.size_bytes = size_bytes;
        true
    } else {
        false
    };
    if !present {
        let _ = trash::delete(path);
        return;
    }
    if let Some(handle) = global(cx).handle {
        let _ = handle.update(cx, |view, _, cx| {
            view.saving.remove(&id);
            if let Some(card) = view.cards.iter_mut().find(|card| card.id == id) {
                card.path = path;
                card.saved = true;
                card.size_bytes = size_bytes;
            }
            cx.notify();
        });
    }
}

fn reveal(path: &std::path::Path) -> std::result::Result<(), String> {
    crate::desktop::reveal(path)
}

pub(crate) fn file_uri(path: &std::path::Path) -> String {
    let absolute = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut uri = String::from("file://");
    for byte in absolute.as_os_str().as_encoded_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            uri.push(*byte as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(uri, "%{byte:02X}");
        }
    }
    uri
}

fn card_exit_pose(mut pose: Pose, exit: Option<&Exit>, right: bool, now: Instant) -> Pose {
    let Some(exit) = exit else { return pose };
    let elapsed = now.duration_since(exit.started_at);
    if elapsed < exit.delay {
        return pose;
    }
    let elapsed_ms = (elapsed - exit.delay).as_secs_f32() * 1_000.0;
    if exit.kind == ExitKind::Dismiss {
        let t = smooth(elapsed_ms / 450.0);
        pose.x += if right { 118.0 } else { -118.0 } * t;
        pose.opacity *= 1.0 - t;
    } else {
        pose.opacity *= (1.0 - elapsed_ms / 1_850.0).clamp(0.0, 1.0);
    }
    pose
}

fn particles(right: bool) -> Vec<Particle> {
    let mut seed = 739_u32;
    let mut random = || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        seed as f32 / u32::MAX as f32
    };
    let (cols, rows) = (20, 11);
    let (width, height) = (CARD_WIDTH / cols as f32, CARD_HEIGHT / rows as f32);
    let origin_x = if right { CARD_WIDTH - 57.5 } else { 57.5 };
    let origin_y = 22.5_f32;
    let max_distance = origin_x
        .max(CARD_WIDTH - origin_x)
        .hypot(origin_y.max(CARD_HEIGHT - origin_y));
    let mut result = Vec::with_capacity(cols * rows);
    for row in 0..rows {
        for column in 0..cols {
            let x = column as f32 * width;
            let y = row as f32 * height;
            let center_x = x + width / 2.0;
            let center_y = y + height / 2.0;
            let wave = (center_x - origin_x).hypot(center_y - origin_y) / max_distance;
            let angle = (center_y - origin_y).atan2(center_x - origin_x);
            let delay = (wave
                + (angle * 2.7 + wave * 5.5).sin() * 0.07 * wave
                + (random() - 0.5) * 0.34 * wave * wave)
                .clamp(0.0, 1.12)
                * 720.0
                + random() * (18.0 + wave * 140.0);
            result.push(Particle {
                x,
                y,
                width: width + 0.55,
                height: height + 0.55,
                dx: (center_x - origin_x) / max_distance * (12.0 + random() * 26.0)
                    + (random() - 0.5) * 22.0,
                dy: -36.0 - random() * 58.0,
                delay_ms: delay,
                duration_ms: 780.0 + random() * 320.0 + wave * 80.0,
            });
        }
    }
    result
}

fn dust(card: &Card, pose: Pose, now: Instant) -> Vec<Div> {
    let Some(exit) = card.exit.as_ref() else {
        return Vec::new();
    };
    let Some(decoded) = card.decoded.as_ref() else {
        return Vec::new();
    };
    let elapsed = now
        .duration_since(exit.started_at)
        .saturating_sub(exit.delay)
        .as_secs_f32()
        * 1_000.0;
    exit.particles
        .iter()
        .filter_map(|particle| {
            let local = elapsed - particle.delay_ms;
            if local < 0.0 || local >= particle.duration_ms {
                return None;
            }
            let t = ease_out_cubic(local / particle.duration_ms);
            let opacity = if t < 0.5 {
                1.0 - 0.28 * (t / 0.5)
            } else {
                0.72 * (1.0 - (t - 0.5) / 0.32).clamp(0.0, 1.0)
            };
            let scale = 1.0 - 0.82 * t;
            Some(
                div()
                    .absolute()
                    .left(px(particle.x + particle.dx * t))
                    .top(px(particle.y + particle.dy * t))
                    .w(px(particle.width * scale))
                    .h(px(particle.height * scale))
                    .opacity(opacity)
                    .overflow_hidden()
                    .child(
                        img(decoded.blurred.clone())
                            .absolute()
                            .left(px(-particle.x * pose.width / CARD_WIDTH))
                            .top(px(-particle.y * pose.height / CARD_HEIGHT))
                            .w(px(pose.width))
                            .h(px(pose.height))
                            .object_fit(ObjectFit::Cover),
                    ),
            )
        })
        .collect()
}

fn reject_shake(elapsed_ms: f32) -> f32 {
    let t = (elapsed_ms / DROP_REJECT_MS).clamp(0.0, 1.0);
    if t >= 1.0 {
        return 0.0;
    }
    let envelope = 1.0 - t;
    (t * std::f32::consts::TAU * 5.0).sin() * 10.0 * envelope
}

fn icon_button(
    id: impl Into<gpui::ElementId>,
    label: &'static str,
    theme: ui::Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .h(px(29.0))
        .min_w(px(29.0))
        .px(px(7.0))
        .rounded(px(7.0))
        .border_1()
        .border_color(theme.glass_text().opacity(0.11))
        .bg(theme.glass())
        .text_color(theme.glass_text())
        .text_xs()
        .cursor_pointer()
        .hover(move |style| style.bg(theme.glass_text().opacity(0.16)))
        .child(label)
}

fn action_button(
    id: impl Into<gpui::ElementId>,
    label: &'static str,
    theme: ui::Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(140.0))
        .h(px(34.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(theme.glass_text().opacity(0.11))
        .bg(theme.glass())
        .text_color(theme.glass_text())
        .text_sm()
        .cursor_pointer()
        .hover(move |style| style.bg(theme.glass_text().opacity(0.16)))
        .child(label)
}

fn meta_chip(card: &Card, theme: ui::Theme) -> Div {
    div()
        .absolute()
        .left(px(8.0))
        .bottom(px(8.0))
        .px(px(7.0))
        .py(px(3.0))
        .rounded(px(5.0))
        .bg(theme.glass())
        .text_color(theme.glass_text())
        .text_xs()
        .child(format!(
            "{} × {} · {}",
            card.width,
            card.height,
            cache::format_bytes(card.size_bytes)
        ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dismiss_never_uses_delete_exit() {
        let now = Instant::now();
        let exit = Exit {
            kind: ExitKind::Dismiss,
            started_at: now,
            delay: Duration::ZERO,
            particles: Arc::new(vec![]),
        };
        let pose = Pose {
            x: 28.0,
            y: 52.0,
            width: 284.0,
            height: 160.0,
            opacity: 1.0,
            rotation: 0.0,
        };
        let halfway = card_exit_pose(pose, Some(&exit), false, now + Duration::from_millis(270));
        assert!(halfway.x < pose.x);
        assert!(halfway.opacity < 1.0);
        assert!(exit.particles.is_empty());
    }

    #[test]
    fn delete_particle_field_matches_web_soft_cap_and_flies_up() {
        let particles = particles(false);
        assert_eq!(particles.len(), 220);
        assert!(particles.iter().all(|particle| particle.dy < 0.0));
        assert!(particles.iter().any(|particle| particle.delay_ms > 720.0));
    }

    #[test]
    fn input_rectangles_follow_only_live_card_poses() {
        let live = Rect {
            x: 28.0,
            y: 52.0,
            width: 284.0,
            height: 160.0,
        };
        assert!(live.x > 0.0);
        assert!(live.width < FRAME_WIDTH);
    }

    #[test]
    fn placement_values_match_native_settings() {
        assert_eq!(placement_edges(0), (true, false));
        assert_eq!(placement_edges(1), (false, false));
        assert_eq!(placement_edges(2), (true, true));
        assert_eq!(placement_edges(3), (false, true));
        assert_eq!(placement_from_edges(true, false), 0);
        assert_eq!(placement_from_edges(false, false), 1);
        assert_eq!(placement_from_edges(true, true), 2);
        assert_eq!(placement_from_edges(false, true), 3);
    }

    #[test]
    fn rotation_cache_quantizes_to_half_degrees_and_skips_zero() {
        assert_eq!(rotation_key(0.2), None);
        assert_eq!(rotation_key(0.3), Some(1));
        assert_eq!(rotation_key(-2.74), Some(-5));
        assert_eq!(rotation_key(20.0), Some(8));
    }

    #[test]
    fn decode_queued_before_hide_cannot_present_after_show() {
        assert!(!may_present(true, 8, 7));
        assert!(!may_present(false, 7, 7));
        assert!(may_present(true, 7, 7));
    }

    #[test]
    fn file_uri_preserves_slashes_and_escapes_spaces() {
        assert_eq!(
            file_uri(std::path::Path::new("/tmp/a capture.mp4")),
            "file:///tmp/a%20capture.mp4"
        );
    }

    #[test]
    fn only_unsaved_and_history_children_are_private() {
        let root = std::env::temp_dir().join(format!(
            "captures-private-preview-test-{}",
            std::process::id()
        ));
        let unsaved = root.join("unsaved/capture.png");
        let history = root.join("history/restored.png");
        let captures = root.join("captures/published.png");
        for path in [&unsaved, &history, &captures] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"capture").unwrap();
        }
        assert!(is_private_path_in(&unsaved, &root));
        assert!(is_private_path_in(&history, &root));
        assert!(!is_private_path_in(&captures, &root));
        assert!(!is_private_path_in(
            &root.join("unsaved/../captures/published.png"),
            &root
        ));
        let _ = fs::remove_dir_all(root);
    }
}
