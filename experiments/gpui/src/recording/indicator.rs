use super::Rect;
use crate::theme::{self, Theme};
use anyhow::Result;
use gpui::{prelude::*, *};

pub(super) struct Indicator {
    hole: Rect,
}

fn outside_rectangles(width: f32, height: f32, hole: Rect) -> [Rect; 4] {
    [
        Rect {
            x: 0.,
            y: 0.,
            width,
            height: hole.y.max(0.),
        },
        Rect {
            x: 0.,
            y: (hole.y + hole.height).min(height),
            width,
            height: (height - hole.y - hole.height).max(0.),
        },
        Rect {
            x: 0.,
            y: hole.y.max(0.),
            width: hole.x.max(0.),
            height: hole.height.min(height),
        },
        Rect {
            x: (hole.x + hole.width).min(width),
            y: hole.y.max(0.),
            width: (width - hole.x - hole.width).max(0.),
            height: hole.height.min(height),
        },
    ]
}

impl Render for Indicator {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let size = window.viewport_size();
        let width = f32::from(size.width);
        let height = f32::from(size.height);
        let t = Theme::for_media(cx);
        let mut root = div().size_full().font_family(theme::font());
        for (index, rect) in outside_rectangles(width, height, self.hole)
            .into_iter()
            .enumerate()
        {
            if rect.width > 0. && rect.height > 0. {
                root = root.child(
                    div()
                        .id(("indicator-dim", index))
                        .absolute()
                        .left(px(rect.x))
                        .top(px(rect.y))
                        .w(px(rect.width))
                        .h(px(rect.height))
                        .bg(rgba(0x06070a66)),
                );
            }
        }
        // Every frame pixel is outside the hole; never use an inset border.
        let h = self.hole;
        root.child(
            div()
                .absolute()
                .left(px(h.x - 2.))
                .top(px(h.y - 2.))
                .w(px(h.width + 4.))
                .h(px(2.))
                .bg(t.accent),
        )
        .child(
            div()
                .absolute()
                .left(px(h.x - 2.))
                .top(px(h.y + h.height))
                .w(px(h.width + 4.))
                .h(px(2.))
                .bg(t.accent),
        )
        .child(
            div()
                .absolute()
                .left(px(h.x - 2.))
                .top(px(h.y))
                .w(px(2.))
                .h(px(h.height))
                .bg(t.accent),
        )
        .child(
            div()
                .absolute()
                .left(px(h.x + h.width))
                .top(px(h.y))
                .w(px(2.))
                .h(px(h.height))
                .bg(t.accent),
        )
    }
}

pub(super) fn open(
    hole: Rect,
    bounds: Bounds<Pixels>,
    cx: &mut App,
) -> Result<WindowHandle<Indicator>> {
    let class = format!("captures-gpui-recording-region-{}", uuid::Uuid::new_v4());
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        anyhow::bail!(
            "recording region guide is unavailable on Wayland because GPUI cannot create a passive shaped window"
        );
    }
    let handle = cx.open_window(
        WindowOptions {
            app_id: Some(class.clone()),
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            window_decorations: Some(WindowDecorations::Client),
            window_background: WindowBackgroundAppearance::Transparent,
            focus: false,
            is_movable: false,
            is_resizable: false,
            ..Default::default()
        },
        |_, cx| cx.new(|_| Indicator { hole }),
    )?;
    let configured = (|| -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            let width = f32::from(bounds.size.width);
            let height = f32::from(bounds.size.height);
            let native = outside_rectangles(width, height, hole)
                .into_iter()
                .filter(|r| r.width > 0. && r.height > 0.)
                .map(|r| {
                    (
                        r.x.round() as i16,
                        r.y.round() as i16,
                        r.width.round() as u16,
                        r.height.round() as u16,
                    )
                })
                .collect::<Vec<_>>();
            crate::integration::configure_x11_region_indicator(&class, bounds, &native)?;
        }
        #[cfg(target_os = "windows")]
        handle.update(cx, |_, window, _| {
            crate::integration::set_window_capture_excluded(window, true)?;
            crate::integration::set_window_mouse_passthrough(window)
        })??;
        #[cfg(target_os = "macos")]
        handle.update(cx, |_, window, _| {
            crate::integration::set_window_mouse_passthrough(window)
        })??;
        crate::refresh_window(handle.into(), cx)?;
        Ok(())
    })();
    if let Err(error) = configured {
        // A handle is not RAII ownership of a GPUI window. Never leave an
        // unconfigured full-screen input surface alive on a setup failure.
        let _ = handle.update(cx, |_, window, _| window.remove_window());
        return Err(error);
    }
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    #[test]
    fn asymmetric_hole_produces_only_outside_geometry() {
        let hole = Rect {
            x: 73.,
            y: 41.,
            width: 311.,
            height: 207.,
        };
        let [top, bottom, left, right] = outside_rectangles(800., 600., hole);
        assert_eq!((top.width, top.height), (800., 41.));
        assert_eq!((bottom.y, bottom.height), (248., 352.));
        assert_eq!((left.width, left.height), (73., 207.));
        assert_eq!((right.x, right.width), (384., 416.));
    }
}
