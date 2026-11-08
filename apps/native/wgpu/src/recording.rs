//! Host hooks for the shared recording worker: tool discovery, capture
//! cancellation and waking the root viewport even while an editor is active.
pub use captures_recording_platform::worker::{Command, Event, Worker};
use eframe::egui;

pub fn new(ctx: egui::Context) -> Worker {
    Worker::new(
        crate::media_tools::locate,
        move || ctx.request_repaint_of(egui::ViewportId::ROOT),
        captures_app::capture_flow::is_current,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn worker_wakes_root_while_an_editor_is_active() {
        let ctx = egui::Context::default();
        let wakes = crate::root_repaint::observe_from_child(&ctx);
        let worker = new(ctx.clone());
        worker.send(Command::Retire);
        assert_eq!(
            wakes.recv_timeout(Duration::from_secs(5)).unwrap(),
            egui::ViewportId::ROOT
        );
        assert!(matches!(worker.try_recv(), Some(Event::Retired)));
        ctx.end_pass().textures_delta.clear();
    }
}
