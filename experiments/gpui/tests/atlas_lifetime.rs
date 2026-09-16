//! Native rendered resource-lifetime stress test. Counters are atlas ownership,
//! not process RSS or physical GPU residency; this is not an FPS benchmark.
#[path = "../src/transient_images.rs"]
mod transient_images;

use gpui::*;
use std::{sync::Arc, time::Duration};

struct Surface {
    anchor: Option<Arc<RenderImage>>,
    image: Option<Arc<RenderImage>>,
    transient: transient_images::TransientImages,
    draws: usize,
}

impl Render for Surface {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.transient.begin_scene(window);
        self.draws += 1;
        div()
            .size_full()
            .bg(rgb(0x20242b))
            .children(
                self.anchor
                    .clone()
                    .map(|image| img(image).w(px(32.)).h(px(32.))),
            )
            .children(
                self.image
                    .clone()
                    .map(|image| img(self.transient.retain(image)).w(px(524.)).h(px(400.))),
            )
    }
}

fn image(width: u32, height: u32, frame: usize) -> Arc<RenderImage> {
    let pixels = image::RgbaImage::from_fn(width, height, |x, y| {
        // Asymmetric changing pixels, including alpha, not a zero-filled upload.
        image::Rgba([
            (x + frame as u32) as u8,
            y as u8,
            173,
            128 + (x % 128) as u8,
        ])
    });
    Arc::new(RenderImage::new([image::Frame::new(pixels)]))
}

async fn wait_draw(handle: WindowHandle<Surface>, target: usize, cx: &mut AsyncApp) -> AtlasStats {
    for _ in 0..250 {
        Timer::after(Duration::from_millis(8)).await;
        let (draws, stats) = handle
            .update(cx, |surface, window, _| {
                (
                    surface.draws,
                    window.atlas_stats().expect("atlas diagnostics supported"),
                )
            })
            .unwrap();
        if draws >= target {
            return stats;
        }
    }
    panic!("native window did not draw the requested scene");
}

async fn wait_retired(
    handle: WindowHandle<Surface>,
    target: usize,
    cx: &mut AsyncApp,
) -> AtlasStats {
    let mut stats = wait_draw(handle, target, cx).await;
    // Blade retires through the submission flushing queued uploads. Empty
    // replacement scenes reach its existing GPU-completion wait boundary.
    for _ in 0..20 {
        if stats.retired_tiles == 0 {
            return stats;
        }
        let target = handle
            .update(cx, |surface, _, cx| {
                cx.notify();
                surface.draws + 1
            })
            .unwrap();
        stats = wait_draw(handle, target, cx).await;
    }
    panic!("retirement did not drain: {stats:?}");
}

fn main() {
    if cfg!(target_os = "windows")
        || (cfg!(target_os = "linux") && std::env::var_os("CAPTURES_GPUI_NATIVE_TEST").is_none())
    {
        println!(
            "atlas_lifetime: skipped (macOS, or Linux with CAPTURES_GPUI_NATIVE_TEST=1 and a graphical session)"
        );
        return;
    }
    Application::new().run(|cx| {
        let anchor = image(32, 32, 0);
        let handle = cx.open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(100.5), px(198.5)), size(px(640.), px(720.)),
            ))),
            titlebar: None,
            kind: WindowKind::PopUp,
            ..Default::default()
        }, |_, cx| cx.new(|_| Surface {
            anchor: Some(anchor.clone()), image: None,
            transient: Default::default(), draws: 0,
        })).unwrap();
        cx.spawn(async move |cx| {
            // Also nudges the known software-X11 initial presentation problem.
            handle.update(cx, |_, window, _| window.resize(size(px(639.), px(719.)))).unwrap();
            Timer::after(Duration::from_millis(100)).await;
            handle.update(cx, |_, window, cx| {
                window.resize(size(px(640.), px(720.)));
                cx.notify();
            }).unwrap();
            let baseline = wait_draw(handle, 1, cx).await;
            assert_eq!(baseline.live_keys, 1);
            let mut max_tiles = 0;
            let mut max_bytes = 0;
            for frame in 0..512 {
                let (width, height) = if frame % 128 < 64 { (1048, 800) } else { (524, 400) };
                let fresh = image(width, height, frame);
                let (target, old) = handle.update(cx, |surface, _, cx| {
                    let old = surface.image.replace(fresh);
                    cx.notify();
                    (surface.draws + 1, old)
                }).unwrap();
                let stats = wait_draw(handle, target, cx).await;
                assert_eq!(stats.live_keys, 2, "only static anchor and current frame: {stats:?}");
                let active = stats.tile_allocations - stats.tile_reclamations;
                assert!(active <= 16, "allocations must not accumulate: {stats:?}");
                assert!(stats.texture_bytes <= 64 * 1024 * 1024, "{stats:?}");
                max_tiles = max_tiles.max(active);
                max_bytes = max_bytes.max(stats.texture_bytes);
                if let Some(old) = old {
                    let weak = Arc::downgrade(&old);
                    handle.update(cx, |_, window, _| {
                        // Repeated eviction must not decrement a shared texture
                        // twice or affect the anchor/current frame.
                        window.drop_image(old.clone()).unwrap();
                        window.drop_image(old.clone()).unwrap();
                        assert_eq!(window.atlas_stats().unwrap().live_keys, 2);
                    }).unwrap();
                    drop(old);
                    assert!(weak.upgrade().is_none(), "old CPU raster retained after scene replacement");
                }
                if frame % 128 == 127 {
                    println!("atlas_lifetime: frame={} stats={stats:?}", frame + 1);
                }
            }
            // The final scene remains valid while idle; only replacing it permits eviction.
            Timer::after(Duration::from_millis(200)).await;
            handle.update(cx, |_, window, _| assert_eq!(window.atlas_stats().unwrap().live_keys, 2)).unwrap();
            let target = handle.update(cx, |surface, _, cx| {
                surface.image = None;
                cx.notify();
                surface.draws + 1
            }).unwrap();
            let stats = wait_retired(handle, target, cx).await;
            assert_eq!(stats.live_keys, baseline.live_keys, "{stats:?}");
            assert_eq!(stats.live_textures, baseline.live_textures, "{stats:?}");
            assert_eq!(stats.allocated_pixels, baseline.allocated_pixels,
                "shared atlas must reclaim holes while the anchor remains: {stats:?}");
            println!("atlas_lifetime: anchor-only cleanup={stats:?}");
            let target = handle.update(cx, |surface, window, cx| {
                surface.anchor = None;
                window.drop_image(anchor.clone()).unwrap();
                cx.notify();
                surface.draws + 1
            }).unwrap();
            let stats = wait_retired(handle, target, cx).await;
            assert_eq!(stats.live_keys, 0, "{stats:?}");
            assert_eq!(stats.retired_tiles, 0, "{stats:?}");
            assert_eq!(stats.live_textures, 0, "{stats:?}");
            assert_eq!(stats.texture_bytes, 0, "{stats:?}");
            assert_eq!(stats.tile_allocations, stats.tile_reclamations, "{stats:?}");
            println!("atlas_lifetime: PASS frames=512 max_active_tiles={max_tiles} max_nominal_bytes={max_bytes} cleanup={stats:?}");
            cx.update(|cx| cx.quit()).unwrap();
        }).detach();
    });
}
