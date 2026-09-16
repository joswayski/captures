//! Images owned by one freshly rebuilt scene, not GPUI's persistent atlas cache.
use gpui::{RenderImage, Window};
use std::sync::Arc;

#[derive(Default)]
pub struct TransientImages(Vec<Arc<RenderImage>>);

impl TransientImages {
    /// Call only when replacing the owning scene, never on a presentation-only
    /// callback. The backend retires old tiles after their GPU users finish.
    pub fn begin_scene(&mut self, window: &mut Window) {
        for image in self.0.drain(..) {
            window.drop_image(image).expect("retire transient image");
        }
    }

    /// Retain the current scene's image until that scene is replaced. A frozen
    /// checkpoint or final frame can therefore be presented repeatedly.
    pub fn retain(&mut self, image: Arc<RenderImage>) -> Arc<RenderImage> {
        self.0.push(image.clone());
        image
    }
}
