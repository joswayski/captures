//! Transitions using the shipping shared/design.css motion curve.
use std::time::Instant;

/// Apply animation translation after Taffy snaps static layout. Prepaint moves
/// descendants and hitboxes together; painting uses those same stored bounds.
pub fn translated(x: f32, y: f32, child: impl gpui::IntoElement) -> Translated {
    Translated {
        child: child.into_any_element(),
        offset: gpui::point(gpui::px(x), gpui::px(y)),
    }
}

pub struct Translated {
    child: gpui::AnyElement,
    offset: gpui::Point<gpui::Pixels>,
}

impl gpui::IntoElement for Translated {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl gpui::Element for Translated {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> (gpui::LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: gpui::Bounds<gpui::Pixels>,
        _: &mut (),
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) {
        window.with_element_offset(self.offset, |window| self.child.prepaint(window, cx));
    }

    fn paint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: gpui::Bounds<gpui::Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) {
        self.child.paint(window, cx);
    }
}

/// Retarget from the currently displayed value, including rapid reversals.
pub struct Motion {
    pub from: f32,
    pub to: f32,
    pub started: Instant,
}

impl Motion {
    pub fn value(&self, now: Instant, duration: f32) -> f32 {
        self.from
            + (self.to - self.from)
                * standard_ease((now.duration_since(self.started).as_secs_f32() / duration).min(1.))
    }

    pub fn retarget(&mut self, to: f32, now: Instant, duration: f32) {
        if self.to != to {
            self.from = self.value(now, duration);
            self.to = to;
            self.started = now;
        }
    }
}

// shared/design.css --ease-standard: cubic-bezier(.2,.8,.2,1).
pub fn standard_ease(progress: f32) -> f32 {
    cubic_ease(progress, 0.2, 0.8, 0.2, 1.)
}

/// mini-preview.css stacked translate transition, after its held-layout exit.
pub fn stack_settle_progress(elapsed_ms: f32, delay_ms: f32) -> f32 {
    cubic_ease(
        ((elapsed_ms - delay_ms) / 580.).clamp(0., 1.),
        0.4,
        0.,
        0.2,
        1.,
    )
}

pub fn cubic_ease(progress: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    if progress <= 0. {
        return 0.;
    }
    if progress >= 1. {
        return 1.;
    }
    let (mut lo, mut hi) = (0., 1.);
    for _ in 0..20 {
        let t = (lo + hi) * 0.5;
        let x = 3. * x1 * (1. - t) * (1. - t) * t + 3. * x2 * (1. - t) * t * t + t * t * t;
        if x < progress {
            lo = t;
        } else {
            hi = t;
        }
    }
    let t = (lo + hi) * 0.5;
    3. * y1 * (1. - t) * (1. - t) * t + 3. * y2 * (1. - t) * t * t + t * t * t
}
