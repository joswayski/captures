//! Native, transient floating notices. Recording ownership stays outside this
//! module: callers supply the source-preserving save and reveal operations.
use crate::{
    Launch, integration,
    theme::{Theme, font},
};
use anyhow::Result;
use gpui::{prelude::FluentBuilder, *};
use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant},
};

const FRAME: f32 = 28.;
const SAVED_LIFETIME: Duration = Duration::from_secs(15);
const HIDDEN_LIFETIME: Duration = Duration::from_secs(6);
const LAUNCH_LIFETIME: Duration = Duration::from_secs(5);
const LAUNCH_AFTER_SETUP_LIFETIME: Duration = Duration::from_secs(15);
const TRAY_RETRY_DELAY: Duration = Duration::from_millis(50);
const TRAY_RETRY_ATTEMPTS: u8 = 20;

pub type NoticeFuture<T> = Pin<Box<dyn Future<Output = Result<T>> + Send>>;
pub type NoticeAction<T> = Arc<dyn Fn(PathBuf) -> NoticeFuture<T> + Send + Sync>;

#[derive(Clone)]
pub struct RecordingReady {
    /// The real recording path/state identifier. This module never copies it itself.
    pub path: PathBuf,
    pub permanently_saved: bool,
    pub save: NoticeAction<PathBuf>,
    pub reveal: NoticeAction<()>,
}

#[derive(Clone)]
struct NoticeRegistry {
    entity: Entity<Notice>,
    window: WindowHandle<Notice>,
}
impl Global for NoticeRegistry {}

#[derive(Clone)]
enum Kind {
    Launch {
        shortcut: Vec<String>,
        caret: Option<(bool, f32)>,
        lifetime: Duration,
    },
    Saved(RecordingReady),
    Hidden {
        shortcut: Vec<String>,
        tray_label: String,
    },
}

struct Notice {
    launch: Launch,
    kind: Kind,
    generation: u64,
    arrived: Instant,
    busy: bool,
    error: Option<String>,
}

pub fn shortcut_tokens(shortcut: &str) -> Vec<String> {
    shortcut
        .split('+')
        .map(|key| {
            match key.trim() {
                "CommandOrControl" => {
                    if cfg!(target_os = "macos") {
                        "⌘"
                    } else {
                        "Ctrl"
                    }
                }
                "Command" | "Meta" | "Super" => {
                    if cfg!(target_os = "macos") {
                        "⌘"
                    } else if cfg!(target_os = "windows") {
                        "Win"
                    } else {
                        "Super"
                    }
                }
                "Control" | "Ctrl" => {
                    if cfg!(target_os = "macos") {
                        "⌃"
                    } else {
                        "Ctrl"
                    }
                }
                "Shift" => {
                    if cfg!(target_os = "macos") {
                        "⇧"
                    } else {
                        "Shift"
                    }
                }
                "Alt" | "Option" => {
                    if cfg!(target_os = "macos") {
                        "⌥"
                    } else {
                        "Alt"
                    }
                }
                "PrintScreen" => "PrtScn",
                other => other.strip_prefix("Key").unwrap_or(other),
            }
            .to_owned()
        })
        .collect()
}

/// Show the real five-second quiet-start notice, anchored to the tray where
/// native geometry is available. Linux AppIndicator deliberately falls back.
pub fn show_launch(shortcut: Vec<String>, launch: Launch, cx: &mut App) -> Result<()> {
    show_launch_for(shortcut, LAUNCH_LIFETIME, launch, cx)
}

/// Schedule the real quiet-start notice after the native tray has had a chance
/// to acquire geometry. Unsupported tray backends skip the pointless delay.
pub fn schedule_launch(shortcut: Vec<String>, launch: Launch, cx: &mut App) {
    cx.spawn(async move |cx| {
        if cfg!(any(target_os = "macos", target_os = "windows")) {
            for _ in 0..TRAY_RETRY_ATTEMPTS {
                let anchored = cx
                    .update(|cx| integration::tray_anchor(cx).is_some())
                    .unwrap_or(false);
                if anchored {
                    break;
                }
                Timer::after(TRAY_RETRY_DELAY).await;
            }
        }
        match cx.update(|cx| show_launch(shortcut, launch, cx)) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => eprintln!("Could not show Captures launch notice: {error:#}"),
            Err(error) => eprintln!("Could not schedule Captures launch notice: {error:#}"),
        }
    })
    .detach();
}

/// Show the longer hint used after first-run setup has completed.
pub fn show_launch_after_setup(shortcut: Vec<String>, launch: Launch, cx: &mut App) -> Result<()> {
    show_launch_for(shortcut, LAUNCH_AFTER_SETUP_LIFETIME, launch, cx)
}

fn show_launch_for(
    shortcut: Vec<String>,
    lifetime: Duration,
    launch: Launch,
    cx: &mut App,
) -> Result<()> {
    let placement = launch_placement(cx)?;
    show(
        Kind::Launch {
            shortcut,
            caret: placement.caret,
            lifetime,
        },
        launch,
        Some(placement.bounds),
        cx,
    )
}

struct LaunchPlacement {
    bounds: Bounds<Pixels>,
    /// (caret is on bottom edge, caret center X in window coordinates)
    caret: Option<(bool, f32)>,
}

fn launch_placement(cx: &mut App) -> Result<LaunchPlacement> {
    let screen = cx
        .primary_display()
        .ok_or_else(|| anyhow::anyhow!("no display available"))?
        .bounds();
    let anchor = integration::tray_anchor(cx)
        .map(|anchor| anchor.bounds)
        .filter(|tray| {
            let center = tray.center();
            screen.contains(&center) && tray.size.width > px(0.) && tray.size.height > px(0.)
        });
    let Some(tray) = anchor else {
        let dimensions = size(px(352.), px(110.));
        return Ok(LaunchPlacement {
            bounds: Bounds::new(
                point(
                    screen.origin.x + screen.size.width - dimensions.width - px(18.),
                    screen.origin.y + px(18.),
                ),
                dimensions,
            ),
            caret: None,
        });
    };
    let dimensions = size(px(352.), px(90.));
    let tray_center = tray.center();
    let caret_bottom = tray_center.y > screen.center().y;
    let min_x = screen.origin.x + px(10.);
    let max_x = screen.origin.x + screen.size.width - dimensions.width - px(10.);
    let x = (tray_center.x - dimensions.width / 2.).clamp(min_x.min(max_x), min_x.max(max_x));
    let desired_y = if caret_bottom {
        tray.origin.y - dimensions.height + px(2.)
    } else {
        tray.origin.y + tray.size.height - px(2.)
    };
    let min_y = screen.origin.y + px(10.);
    let max_y = screen.origin.y + screen.size.height - dimensions.height - px(10.);
    let y = desired_y.clamp(min_y.min(max_y), min_y.max(max_y));
    let caret_x = f32::from(tray_center.x - x).clamp(52., 300.);
    Ok(LaunchPlacement {
        bounds: Bounds::new(point(x, y), dimensions),
        caret: Some((caret_bottom, caret_x)),
    })
}

/// Show or replace the recording-ready/saved notice with real state and actions.
pub fn show_recording_ready(state: RecordingReady, launch: Launch, cx: &mut App) -> Result<()> {
    show(Kind::Saved(state), launch, None, cx)
}

/// Show or replace the controls-hidden notice. Tokens are rendered as individual keys.
pub fn show_controls_hidden(
    shortcut: Vec<String>,
    tray_label: impl Into<String>,
    launch: Launch,
    cx: &mut App,
) -> Result<()> {
    show(
        Kind::Hidden {
            shortcut,
            tray_label: tray_label.into(),
        },
        launch,
        None,
        cx,
    )
}

pub fn open_fixture(launch: Launch, cx: &mut App) -> Result<()> {
    anyhow::ensure!(
        launch.mock,
        "notice fixtures require --mock; real notices are driven by the recording lifecycle"
    );
    let shortcut = vec![
        if cfg!(target_os = "macos") {
            "⌘"
        } else {
            "Ctrl"
        }
        .into(),
        "Shift".into(),
        "Space".into(),
    ];
    match launch.view.as_str() {
        "launch-notice" | "startup" => show_launch(shortcut, launch, cx),
        "recording-controls-hidden" => show_controls_hidden(
            shortcut,
            if cfg!(target_os = "macos") {
                "menu bar"
            } else {
                "system tray"
            },
            launch,
            cx,
        ),
        "recording-saved" | "recording-ready" => {
            let saved = launch.view == "recording-saved";
            show_recording_ready(
                RecordingReady {
                    path: launch
                        .path
                        .clone()
                        .unwrap_or_else(|| "Mock recording.mp4".into()),
                    permanently_saved: saved,
                    save: Arc::new(|path| Box::pin(async { Ok(path) })),
                    reveal: Arc::new(|_| Box::pin(async { Ok(()) })),
                },
                launch,
                cx,
            )
        }
        "recording-save-error" => show_recording_ready(
            RecordingReady {
                path: "Mock recording.mp4".into(),
                permanently_saved: false,
                save: Arc::new(|_| Box::pin(async { anyhow::bail!("Mock permission denied") })),
                reveal: Arc::new(|_| Box::pin(async { anyhow::bail!("Mock permission denied") })),
            },
            launch,
            cx,
        ),
        _ => anyhow::bail!("unknown notice fixture"),
    }
}

fn show(
    kind: Kind,
    launch: Launch,
    requested_bounds: Option<Bounds<Pixels>>,
    cx: &mut App,
) -> Result<()> {
    if let Some(registry) = cx.try_global::<NoticeRegistry>().cloned()
        && std::mem::discriminant(&registry.entity.read(cx).kind) == std::mem::discriminant(&kind)
        && registry
            .window
            .update(cx, |_, window, _| {
                window.refresh();
            })
            .is_ok()
    {
        registry.entity.update(cx, |notice, cx| {
            notice.kind = kind;
            notice.launch = launch;
            notice.generation = notice.generation.wrapping_add(1);
            notice.arrived = Instant::now();
            notice.busy = false;
            notice.error = None;
            cx.notify();
        });
        schedule_dismiss(registry.entity, registry.window.into(), cx);
        return Ok(());
    }
    if let Some(registry) = cx.try_global::<NoticeRegistry>().cloned() {
        let _ = registry
            .window
            .update(cx, |_, window, _| window.remove_window());
    }
    let (dimensions, class) = match kind {
        Kind::Launch { .. } => (size(px(352.), px(90.)), "captures-gpui-launch-notice"),
        Kind::Saved(_) => (size(px(496.), px(172.)), "captures-gpui-recording-saved"),
        Kind::Hidden { .. } => (size(px(474.), px(130.)), "captures-gpui-controls-hidden"),
    };
    let display = cx
        .primary_display()
        .ok_or_else(|| anyhow::anyhow!("no display available"))?;
    let screen = display.bounds();
    let bounds = requested_bounds.unwrap_or_else(|| {
        Bounds::new(
            point(
                screen.origin.x + screen.size.width - dimensions.width - px(18.),
                screen.origin.y + px(18.),
            ),
            dimensions,
        )
    });
    let entity = cx.new(|_| Notice {
        launch: launch.clone(),
        kind,
        generation: 1,
        arrived: Instant::now(),
        busy: false,
        error: None,
    });
    let root = entity.clone();
    let window = cx.open_window(
        WindowOptions {
            app_id: Some(class.into()),
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            window_decorations: Some(WindowDecorations::Client),
            window_background: WindowBackgroundAppearance::Transparent,
            kind: WindowKind::PopUp,
            focus: false,
            is_resizable: false,
            is_movable: false,
            is_minimizable: false,
            ..Default::default()
        },
        move |_, _| root,
    )?;
    #[cfg(target_os = "linux")]
    integration::configure_x11_floating(class, bounds)?;
    cx.set_global(NoticeRegistry {
        entity: entity.clone(),
        window,
    });
    if matches!(entity.read(cx).kind, Kind::Launch { .. }) {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        window.update(cx, |_, window, _| {
            integration::set_window_mouse_passthrough(window)
        })??;
        #[cfg(target_os = "linux")]
        if std::env::var_os("WAYLAND_DISPLAY").is_none() {
            integration::configure_x11_region_indicator(
                class,
                bounds,
                &[(0, 0, 352, f32::from(bounds.size.height).round() as u16)],
            )?;
        }
    }
    schedule_dismiss(entity, window.into(), cx);
    crate::refresh_window(window.into(), cx)?;
    Ok(())
}

fn schedule_dismiss(entity: Entity<Notice>, window: AnyWindowHandle, cx: &mut App) {
    let (generation, lifetime) = entity.read(cx).generation_and_lifetime();
    cx.spawn(async move |cx| {
        Timer::after(lifetime).await;
        let remove = entity
            .update(cx, |notice, _| {
                notice.generation == generation && !notice.busy && notice.error.is_none()
            })
            .unwrap_or(false);
        if remove {
            let _ = window.update(cx, |_, window, _| window.remove_window());
        }
    })
    .detach();
}

impl Notice {
    fn generation_and_lifetime(&self) -> (u64, Duration) {
        (
            self.generation,
            match self.kind {
                Kind::Saved(_) => SAVED_LIFETIME,
                Kind::Hidden { .. } => HIDDEN_LIFETIME,
                Kind::Launch { lifetime, .. } => lifetime,
            },
        )
    }
    fn dismiss(&mut self, _: &ClickEvent, window: &mut Window, _: &mut Context<Self>) {
        window.remove_window();
    }
    fn run_action(&mut self, save: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Kind::Saved(state) = &self.kind else {
            return;
        };
        self.busy = true;
        self.error = None;
        let operation: NoticeFuture<Option<PathBuf>> = if save {
            let task = (state.save)(state.path.clone());
            Box::pin(async { task.await.map(Some) })
        } else {
            let task = (state.reveal)(state.path.clone());
            Box::pin(async { task.await.map(|()| None) })
        };
        let handle = window.window_handle();
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let result = operation.await;
            let _ = this.update(cx, |notice, cx| {
                if notice.generation != generation {
                    return;
                }
                notice.busy = false;
                match result {
                    Ok(Some(path)) => {
                        if let Kind::Saved(state) = &mut notice.kind {
                            state.permanently_saved = true;
                            state.path = path;
                        }
                        notice.arrived = Instant::now();
                        notice.generation = notice.generation.wrapping_add(1);
                        let entity = cx.entity();
                        cx.defer(move |cx| schedule_dismiss(entity, handle, cx));
                    }
                    Ok(None) => {
                        cx.defer(move |cx| {
                            let _ = handle.update(cx, |_, window, _| window.remove_window());
                        });
                    }
                    Err(error) => {
                        notice.error = Some(if save {
                            format!("Could not save the recording: {error}")
                        } else {
                            format!("Could not show the recording in its folder: {error}")
                        })
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

fn keys(tokens: &[String], t: Theme) -> Div {
    tokens.iter().fold(div().flex().gap_1(), |row, key| {
        row.child(
            div()
                .px_1()
                .py(px(1.))
                .border_1()
                .border_color(t.glass_border)
                .rounded(px(4.))
                .bg(rgba(0xffffff14))
                .text_color(t.glass_text)
                .font_weight(FontWeight::SEMIBOLD)
                .child(key.clone()),
        )
    })
}

fn caret(bottom: bool) -> Img {
    let path = if bottom {
        "M0 0H18L9 10Z"
    } else {
        "M0 10H18L9 0Z"
    };
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 18 10"><path d="{path}" fill="#0f0f12" fill-opacity=".98"/></svg>"##
    );
    img(Arc::new(Image::from_bytes(
        ImageFormat::Svg,
        svg.into_bytes(),
    )))
    .w(px(18.))
    .h(px(10.))
}

fn icon(body: &str, color: Rgba, dimension: f32) -> Img {
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#{:02x}{:02x}{:02x}" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">{body}</svg>"##,
        (color.r * 255.) as u8,
        (color.g * 255.) as u8,
        (color.b * 255.) as u8
    );
    img(Arc::new(Image::from_bytes(
        ImageFormat::Svg,
        svg.into_bytes(),
    )))
    .size(px(dimension))
}

impl Render for Notice {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let elapsed = self.arrived.elapsed().as_secs_f32();
        let lifetime = self.generation_and_lifetime().1.as_secs_f32();
        let enter_for = match &self.kind {
            Kind::Hidden { .. } => 0.3,
            _ => 0.375,
        };
        let leave_for = match &self.kind {
            Kind::Hidden { .. } => 1.2,
            Kind::Saved(_) => 2.1,
            Kind::Launch { .. } => 0.0,
        };
        let reduced_motion = crate::theme::reduced_motion();
        let entering = if reduced_motion {
            1.
        } else {
            (elapsed / enter_for).min(1.)
        };
        let leaving = if leave_for == 0.0 || reduced_motion || self.busy || self.error.is_some() {
            1.0
        } else {
            ((lifetime - elapsed) / leave_for).clamp(0., 1.)
        };
        // Action failures remain fully visible and actionable until dismissed.
        let opacity = if self.error.is_some() || self.busy {
            1.0
        } else {
            entering.min(leaving)
        };
        if !reduced_motion && elapsed < lifetime && self.error.is_none() && !self.busy {
            window.request_animation_frame();
        }
        let t = Theme::for_window(&self.launch, window, cx);
        let travel: f32 = match &self.kind {
            Kind::Launch {
                caret: Some((true, _)),
                ..
            }
            | Kind::Hidden { .. } => 7.0,
            _ => -8.0,
        };
        let offset = if entering < 1.0 {
            travel * (1.0 - entering)
        } else if leaving < 1.0 {
            travel.signum() * 5.0 * (1.0 - leaving)
        } else {
            0.0
        };
        let base = div()
            .relative()
            .top(px(offset))
            .font_family(font())
            .opacity(opacity)
            .text_color(t.glass_text);
        match &self.kind {
            Kind::Launch {
                shortcut,
                caret: anchor_caret,
                ..
            } => base
                .size_full()
                .relative()
                .when(anchor_caret.is_some_and(|(bottom, _)| !bottom), |d| {
                    let x = anchor_caret.map_or(0., |(_, x)| x);
                    d.child(caret(false).absolute().top(px(0.)).left(px(x - 9.)))
                })
                .child(
                    div()
                        .absolute()
                        .left(px(28.))
                        .top(px(if anchor_caret.is_some_and(|(bottom, _)| !bottom) {
                            8.
                        } else {
                            28.
                        }))
                        .w(px(296.))
                        .h(px(54.))
                        .rounded(px(27.))
                        .bg(rgba(0x0f0f12fa))
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .text_size(px(15.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Captures is ready to use"),
                        )
                        .child(
                            div()
                                .mt_1()
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_size(px(12.))
                                .text_color(t.glass_muted)
                                .child("Open New Capture with")
                                .child(keys(shortcut, t)),
                        ),
                )
                .when(anchor_caret.is_some_and(|(bottom, _)| bottom), |d| {
                    let x = anchor_caret.map_or(0., |(_, x)| x);
                    d.child(caret(true).absolute().bottom(px(0.)).left(px(x - 9.)))
                }),
            Kind::Hidden {
                shortcut,
                tray_label,
            } => base.size_full().p(px(FRAME)).child(
                div()
                    .size_full()
                    .px_5()
                    .border_1()
                    .border_color(t.glass_border)
                    .rounded(px(16.))
                    .bg(t.glass)
                    .shadow_lg()
                    .flex()
                    .items_center()
                    .gap_5()
                    .child(
                        div()
                            .size(px(34.))
                            .flex_shrink_0()
                            .rounded(px(10.))
                            .bg(t.accent)
                            .text_color(rgb(0x101014))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(r#"<path d="M9 4H7a3 3 0 0 0-3 3v2M15 4h2a3 3 0 0 1 3 3v2M20 15v2a3 3 0 0 1-3 3h-2M9 20H7a3 3 0 0 1-3-3v-2"/>"#, rgb(0x101014), 20.)),
                    )
                    .child(
                        div().flex_1().min_w_0()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Recording controls hidden"),
                            )
                            .child(
                                div()
                                    .mt_1()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap_1()
                                    .text_size(px(11.))
                                    .text_color(t.glass_muted)
                                    .child(format!("Open Captures from the {tray_label}, or press"))
                                    .child(keys(shortcut, t))
                                    .child("to bring them back."),
                            ),
                    ),
            ),
            Kind::Saved(state) => {
                let saved = state.permanently_saved;
                let busy = self.busy;
                let error = self.error.clone();
                let action = div().id("recording-action").h(px(32.)).px_3().flex_shrink_0()
                    .border_1().border_color(t.glass_border).rounded(px(8.)).bg(rgba(0xffffff14))
                    .flex().items_center().gap_2().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD)
                    .when(!busy, |d| d.cursor_pointer().on_click(cx.listener(move |s, _, window, cx| s.run_action(!saved, window, cx))))
                    .child(icon(if saved { r#"<path d="M3 7a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v9H3Z"/>"# } else { r#"<path d="M5 4h12l2 2v14H5ZM8 4v6h8V4M8 20v-6h8v6"/>"# }, t.glass_text, 14.))
                    .child(if busy { if saved { "Opening…" } else { "Saving…" } } else if saved { "Show in Folder" } else { "Save file" });
                let copy = div().flex_1().min_w_0()
                    .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(if saved { "Recording saved" } else { "Recording ready" }))
                    .child(div().mt(px(2.)).text_size(px(11.)).text_color(if error.is_some() { t.signal } else { t.glass_muted })
                        .child(error.unwrap_or_else(|| if saved { "Saved to your Captures folder.".into() } else { "Kept in Capture History for 30 days. Save a copy anytime.".into() })));
                base.size_full().p(px(FRAME)).child(
                    div().relative().size_full().pl_5().pr(px(38.)).border_1().border_color(t.glass_border)
                        .rounded(px(16.)).bg(t.glass).shadow_lg().flex().items_center().gap_5()
                        .child(div().size(px(38.)).flex_shrink_0().rounded(px(10.)).bg(t.positive).flex().items_center().justify_center()
                            .child(icon(r#"<path d="m5 12 4 4L19 6"/>"#, t.glass_text, 20.)))
                        .child(copy).child(action)
                        .child(div().absolute().top(px(7.)).right(px(8.)).id("dismiss-notice").size(px(24.)).rounded(px(5.))
                            .flex().items_center().justify_center().text_color(t.glass_muted).cursor_pointer()
                            .on_click(cx.listener(Self::dismiss)).child("×"))
                )
            }
        }
    }
}
