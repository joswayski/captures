//! Native focus generations for asynchronous clipboard consumers. Missing Paste
//! keys are recovered at native translation in eframe, never batched at redraw.
#[cfg(target_os = "linux")]
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use winit::{event::WindowEvent, window::WindowId};

#[cfg(target_os = "linux")]
pub const FOCUS_EPOCH: &str = "native-paste-focus-epoch";

#[derive(Clone, Default)]
pub struct PasteInput {
    #[cfg(target_os = "linux")]
    focus_epoch: Arc<AtomicU64>,
}

impl PasteInput {
    pub fn begin_event(&self, _window: WindowId, event: &WindowEvent) {
        // A blur/refocus pair may arrive between redraws. RawInput.focused
        // alone cannot detect that an asynchronous paste lost its owner.
        #[cfg(target_os = "linux")]
        if matches!(event, WindowEvent::Focused(_) | WindowEvent::Destroyed) {
            self.focus_epoch.fetch_add(1, Ordering::Relaxed);
        }
        let _ = event;
    }

    pub fn publish(&self, ctx: &eframe::egui::Context) {
        #[cfg(target_os = "linux")]
        ctx.data_mut(|data| {
            data.insert_temp(
                eframe::egui::Id::unique(FOCUS_EPOCH),
                self.focus_epoch.load(Ordering::Relaxed),
            )
        });
        let _ = ctx;
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn native_blur_refocus_between_redraws_changes_the_paste_epoch() {
        let bridge = PasteInput::default();
        let window = WindowId::from(1);
        let ctx = eframe::egui::Context::default();
        bridge.begin_event(window, &WindowEvent::Focused(false));
        bridge.begin_event(window, &WindowEvent::Focused(true));
        bridge.begin_event(window, &WindowEvent::RedrawRequested);
        bridge.publish(&ctx);
        assert_eq!(
            ctx.data(|data| data.get_temp::<u64>(eframe::egui::Id::unique(FOCUS_EPOCH))),
            Some(2)
        );
    }
}
