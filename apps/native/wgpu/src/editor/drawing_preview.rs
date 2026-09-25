//! One in-flight render plus one replaceable pending gesture. Replies never
//! publish editor state, and cancellation invalidates both queued and late work.
use super::{Job, Request, Sender, egui, mpsc, wake};
use image::RgbaImage;
use std::sync::Arc;

pub(super) struct Reply {
    pub epoch: u64,
    pub pixels: Result<Arc<RgbaImage>, String>,
}

pub(super) struct State {
    ctx: egui::Context,
    viewport: egui::ViewportId,
    epoch: u64,
    in_flight: bool,
    latest: Option<Request>,
    reply: Sender<Reply>,
    replies: mpsc::Receiver<Reply>,
    pub texture: Option<egui::TextureHandle>,
}

impl State {
    pub fn new(ctx: egui::Context, viewport: egui::ViewportId) -> Self {
        let (reply, replies) = mpsc::channel();
        Self {
            ctx,
            viewport,
            epoch: 0,
            in_flight: false,
            latest: None,
            reply,
            replies,
            texture: None,
        }
    }

    pub fn request(&mut self, tx: &Sender<Job>, request: Request) {
        self.latest = Some(request);
        self.dispatch(tx);
    }

    fn dispatch(&mut self, tx: &Sender<Job>) {
        if !self.in_flight
            && let Some(request) = self.latest.take()
        {
            self.in_flight = tx
                .send(Job::DrawingPreview {
                    request,
                    epoch: self.epoch,
                    reply: self.reply.clone(),
                })
                .is_ok();
        }
    }

    pub fn cancel(&mut self) {
        if self.in_flight || self.latest.is_some() || self.texture.is_some() {
            self.epoch += 1;
            self.latest = None;
            self.texture = None;
            wake(&self.ctx, self.viewport);
        }
    }

    pub fn receive(&mut self, tx: &Sender<Job>) {
        while let Ok(reply) = self.replies.try_recv() {
            self.in_flight = false;
            if reply.epoch == self.epoch {
                self.texture = reply.pixels.ok().map(|pixels| {
                    self.ctx.load_texture(
                        "drawing-preview",
                        egui::ColorImage::from_rgba_unmultiplied(
                            [pixels.width() as usize, pixels.height() as usize],
                            pixels.as_raw(),
                        ),
                        egui::TextureOptions::LINEAR,
                    )
                });
                self.ctx.request_repaint_of(self.viewport);
            }
        }
        self.dispatch(tx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(x: f64) -> Request {
        serde_json::from_value(serde_json::json!({"operation":"create_open_shape",
            "shape":"line", "start":{"x":x,"y":2}, "end":{"x":12,"y":8}}))
        .unwrap()
    }

    fn take(rx: &mpsc::Receiver<Job>) -> (u64, Sender<Reply>, f64) {
        let Job::DrawingPreview {
            epoch,
            reply,
            request: Request::CreateOpenShape { create },
        } = rx.try_recv().unwrap()
        else {
            panic!("expected drawing preview")
        };
        (epoch, reply, create.start.x)
    }

    #[test]
    fn coalesces_work_and_rejects_late_results_after_cancel_or_next_gesture() {
        let ctx = egui::Context::default();
        let mut state = State::new(ctx, egui::ViewportId::ROOT);
        let (tx, rx) = mpsc::channel();
        state.request(&tx, request(1.));
        let (old_epoch, reply, x) = take(&rx);
        assert_eq!(x, 1.);
        state.request(&tx, request(3.));
        state.request(&tx, request(5.));
        assert!(
            rx.try_recv().is_err(),
            "one render in flight regardless of pointer rate"
        );
        let pixels = Arc::new(RgbaImage::new(7, 3));
        reply
            .send(Reply {
                epoch: old_epoch,
                pixels: Ok(pixels.clone()),
            })
            .unwrap();
        state.receive(&tx);
        assert!(state.texture.is_some());
        let (epoch, reply, x) = take(&rx);
        assert_eq!(x, 5., "skip the obsolete intermediate geometry");
        state.cancel();
        state.request(&tx, request(9.));
        assert!(state.texture.is_none());
        assert!(
            rx.try_recv().is_err(),
            "new gesture cannot grow the worker queue"
        );
        reply
            .send(Reply {
                epoch,
                pixels: Ok(pixels.clone()),
            })
            .unwrap();
        state.receive(&tx);
        assert!(
            state.texture.is_none(),
            "cancelled frame cannot replace the new gesture"
        );
        let (epoch, reply, x) = take(&rx);
        assert_eq!(x, 9.);
        reply
            .send(Reply {
                epoch,
                pixels: Ok(pixels),
            })
            .unwrap();
        state.receive(&tx);
        assert_eq!(state.texture.as_ref().unwrap().size(), [7, 3]);
        state.receive(&tx);
        assert!(rx.try_recv().is_err(), "no recurring static render");
        state.cancel();
        assert!(state.texture.is_none());
    }
}
