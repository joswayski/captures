use crate::theme::{self, Theme};
use gpui::{prelude::*, *};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub(super) struct RestartCountdown {
    remaining: u8,
    started: Instant,
    exiting: bool,
    cancelled: Arc<AtomicBool>,
    focus: FocusHandle,
}

impl Render for RestartCountdown {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::new(false);
        let width = f32::from(window.viewport_size().width);
        let elapsed = self.started.elapsed().as_secs_f32();
        let (opacity, scale) =
            super::countdown_pose(elapsed, self.exiting, theme::reduced_motion());
        if elapsed < if self.exiting { 0.14 } else { 0.28 } {
            window.request_animation_frame();
        }
        div()
            .size_full()
            .bg(rgba(0x06070a80))
            .opacity(opacity)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, _| {
                if event.keystroke.key == "escape" {
                    this.cancelled.store(true, Ordering::Release);
                    window.remove_window();
                }
            }))
            .flex()
            .items_center()
            .justify_center()
            .text_color(t.glass_text)
            .font_family(theme::font())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(
                        div()
                            .text_color(t.glass_muted)
                            .font_weight(FontWeight::MEDIUM)
                            .text_size(px((width * 0.016).clamp(14., 20.)))
                            .child("RECORDING STARTS IN"),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .text_size(px((width * 0.26).clamp(150., 340.) * scale))
                            .line_height(relative(0.95))
                            .my(px(14.))
                            .child(self.remaining.to_string()),
                    )
                    .child(
                        div()
                            .mt(px(28.))
                            .text_size(px(14.))
                            .text_color(t.glass_muted)
                            .child("Press Esc to cancel"),
                    ),
            )
    }
}

pub(super) fn open(
    bounds: Bounds<Pixels>,
    seconds: u8,
    cx: &mut App,
    finished: impl FnOnce(bool, &mut App) + Send + 'static,
) -> anyhow::Result<()> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let state = cancelled.clone();
    let class = format!("captures-gpui-restart-countdown-{}", uuid::Uuid::new_v4());
    let handle = cx.open_window(
        WindowOptions {
            app_id: Some(class.clone()),
            window_bounds: Some(WindowBounds::Fullscreen(bounds)),
            titlebar: None,
            window_decorations: Some(WindowDecorations::Client),
            window_background: WindowBackgroundAppearance::Transparent,
            is_movable: false,
            is_resizable: false,
            ..Default::default()
        },
        |window, cx| {
            let focus = cx.focus_handle();
            focus.focus(window);
            cx.new(|_| RestartCountdown {
                remaining: seconds,
                started: Instant::now(),
                exiting: false,
                cancelled,
                focus,
            })
        },
    )?;
    #[cfg(target_os = "linux")]
    if let Err(error) = crate::integration::configure_x11_floating(&class, bounds) {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
        return Err(error);
    }
    crate::present_window(handle.into(), cx)?;
    cx.spawn(async move |cx| {
        for remaining in (1..=seconds).rev() {
            Timer::after(Duration::from_secs(1)).await;
            if state.load(Ordering::Acquire) || !captures_session::capture_session_available() {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
                let _ = cx.update(|cx| finished(false, cx));
                return;
            }
            if handle
                .update(cx, |s, _, cx| {
                    s.remaining = remaining.saturating_sub(1).max(1);
                    s.exiting = remaining == 1;
                    s.started = Instant::now();
                    cx.notify();
                })
                .is_err()
            {
                let _ = cx.update(|cx| finished(false, cx));
                return;
            }
        }
        Timer::after(Duration::from_millis(if theme::reduced_motion() {
            0
        } else {
            140
        }))
        .await;
        let complete =
            !state.load(Ordering::Acquire) && captures_session::capture_session_available();
        // A native close is cancellation too, even during the final exit frame.
        let present = handle.update(cx, |_, w, _| w.remove_window()).is_ok();
        let _ = cx.update(|cx| finished(complete && present, cx));
    })
    .detach();
    Ok(())
}
