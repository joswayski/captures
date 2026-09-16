//! Native tab stops and focus visibility for the shipping History button order.
use crate::theme::Theme;
use gpui::{prelude::*, *};
use std::{cell::Cell, collections::HashMap, rc::Rc};

#[derive(Default)]
pub(super) struct HistoryControls {
    handles: HashMap<ElementId, (FocusHandle, bool)>,
    pub scroll: ScrollHandle,
    keyboard: bool,
    reveal: Rc<Cell<bool>>,
}

impl HistoryControls {
    pub fn begin(&mut self) {
        for (_, live) in self.handles.values_mut() {
            *live = false;
        }
    }

    pub fn finish(&mut self, root: &FocusHandle, window: &mut Window) {
        // Filtering, disabling or deleting the focused control must not leave
        // the keyboard attached to an element absent from the dispatch tree.
        self.handles.retain(|_, (handle, live)| {
            if !*live && handle.is_focused(window) {
                root.focus(window);
            }
            *live
        });
    }

    pub fn pointer(&mut self) {
        self.keyboard = false;
        self.reveal.set(false);
    }

    pub fn tab(&mut self, backwards: bool, window: &mut Window) {
        self.keyboard = true;
        self.reveal.set(true);
        if backwards {
            window.focus_prev();
        } else {
            window.focus_next();
        }
    }

    pub fn decorate(
        &mut self,
        button: Stateful<Div>,
        id: impl Into<ElementId>,
        disabled: bool,
        radius: Option<f32>,
        t: Theme,
        cx: &mut App,
    ) -> Stateful<Div> {
        if disabled {
            return button;
        }
        let (focus, live) = self
            .handles
            .entry(id.into())
            .or_insert_with(|| (cx.focus_handle().tab_stop(true), true));
        *live = true;
        let focus = focus.clone();
        let painted_focus = focus.clone();
        let scroll = self.scroll.clone();
        let reveal = self.reveal.clone();
        let keyboard = self.keyboard;
        // Keep geometry and tab order in GPUI's actual element tree; no second
        // list of actions that can drift from conditional/disabled controls.
        button.track_focus(&focus).child(
            canvas(
                move |bounds, window, _| {
                    let focused = keyboard && focus.is_focused(window);
                    if focused && reveal.replace(false) {
                        let delta = reveal_delta(bounds, scroll.bounds());
                        if delta != px(0.) {
                            let offset = scroll.offset();
                            scroll.set_offset(point(offset.x, offset.y + delta));
                            window.refresh();
                        }
                    }
                },
                move |bounds, _, window, _| {
                    if !keyboard || !painted_focus.is_focused(window) {
                        return;
                    }
                    let mut accent = t.accent;
                    accent.a = 0.55;
                    // Match --focus-ring without changing layout or hitboxes.
                    // Preview media needs the inset variant inside its clip.
                    for (bounds, radius, color) in if let Some(radius) = radius {
                        [
                            (bounds.dilate(px(4.)), radius + 4., accent),
                            (bounds.dilate(px(2.)), radius + 2., t.canvas),
                        ]
                    } else {
                        [(bounds.inset(px(2.)), 0., accent), (bounds, 0., t.canvas)]
                    } {
                        window.paint_quad(quad(
                            bounds,
                            px(radius),
                            transparent_black(),
                            px(2.),
                            color,
                            BorderStyle::Solid,
                        ));
                    }
                },
            )
            .absolute()
            .inset_0(),
        )
    }
}

fn reveal_delta(control: Bounds<Pixels>, viewport: Bounds<Pixels>) -> Pixels {
    // The canvas is inside the control border. Include that border plus the
    // four-pixel exterior ring and its antialiased edge in the revealed area.
    let top = viewport.top() + px(6.);
    let bottom = viewport.bottom() - px(6.);
    if control.top() < top {
        top - control.top()
    } else if control.bottom() > bottom {
        bottom - control.bottom()
    } else {
        px(0.)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn focus_scroll_moves_only_enough_to_reveal_either_edge_and_ring() {
        let viewport = Bounds::new(point(px(31.), px(70.)), size(px(620.), px(440.)));
        let control = |y, h| Bounds::new(point(px(100.), px(y)), size(px(200.), px(h)));
        assert_eq!(reveal_delta(control(78., 32.), viewport), px(0.));
        assert_eq!(reveal_delta(control(76., 428.), viewport), px(0.));
        assert_eq!(reveal_delta(control(75., 32.), viewport), px(1.));
        assert_eq!(reveal_delta(control(-40., 32.), viewport), px(116.));
        assert_eq!(reveal_delta(control(473., 32.), viewport), px(-1.));
        assert_eq!(reveal_delta(control(702., 168.), viewport), px(-366.));
        // A control taller than the viewport aligns its start rather than
        // oscillating between the two edges on successive frames.
        assert_eq!(reveal_delta(control(55., 700.), viewport), px(21.));
    }
}
