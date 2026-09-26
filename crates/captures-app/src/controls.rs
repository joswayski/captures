//! Behaviour of the shipping UI primitives (`CustomSelect.tsx`,
//! `NumberInput.tsx`, `RangeSlider.tsx`, `lib/customSelectMenu.ts`) for the
//! native hosts: select keyboard navigation and menu placement (wgpu; AppKit
//! menus keep native keys), number stepping (both hosts; AppKit through
//! `captures_controls_v1`), and slider track positions. Hosts only draw.

use serde::Serialize;

/// Shipping `CustomSelect` listbox behaviour.
pub mod select {
    use super::Serialize;

    pub const VIEWPORT_PADDING: f64 = 8.;
    pub const MENU_GAP: f64 = 6.;
    pub const MAX_MENU_HEIGHT: f64 = 240.;
    pub const MAX_MENU_WIDTH: f64 = 360.;

    /// Keys a focused select trigger handles.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Key {
        ArrowDown,
        ArrowUp,
        Home,
        End,
        Enter,
        Space,
        Escape,
    }

    /// Whether the listbox is open and which option is active (highlighted).
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct State {
        pub open: bool,
        pub active: usize,
    }

    /// The result of a key: the new state, a chosen option index, and whether
    /// the select consumed the key (shipping calls `preventDefault`).
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct KeyOutcome {
        pub state: State,
        pub chosen: Option<usize>,
        pub handled: bool,
    }

    fn enabled(disabled: &[bool]) -> Vec<usize> {
        (0..disabled.len())
            .filter(|&index| !disabled[index])
            .collect()
    }

    /// Opening highlights the selected option, or the first enabled one when
    /// the selection is disabled.
    pub fn open(disabled: &[bool], selected: usize) -> State {
        let active = if disabled.get(selected).copied().unwrap_or(true) {
            enabled(disabled).first().copied().unwrap_or(0)
        } else {
            selected
        };
        State { open: true, active }
    }

    /// Move the active option one enabled step, wrapping at either end.
    pub fn move_active(disabled: &[bool], active: usize, down: bool) -> usize {
        let indexes = enabled(disabled);
        if indexes.is_empty() {
            return active;
        }
        let next = match indexes.iter().position(|&index| index == active) {
            None if down => 0,
            None => indexes.len() - 1,
            Some(current) if down => (current + 1) % indexes.len(),
            Some(current) => (current + indexes.len() - 1) % indexes.len(),
        };
        indexes[next]
    }

    /// Shipping trigger `onKeyDown`: arrows open or move, Home/End jump while
    /// open, Enter/Space open or choose, Escape closes an open listbox.
    pub fn key(state: State, disabled: &[bool], selected: usize, key: Key) -> KeyOutcome {
        let outcome = |state, chosen, handled| KeyOutcome {
            state,
            chosen,
            handled,
        };
        let closed = State {
            open: false,
            ..state
        };
        match key {
            Key::Escape if state.open => outcome(closed, None, true),
            Key::ArrowDown | Key::ArrowUp if !state.open => {
                outcome(open(disabled, selected), None, true)
            }
            Key::ArrowDown | Key::ArrowUp => {
                let active = move_active(disabled, state.active, key == Key::ArrowDown);
                outcome(State { active, ..state }, None, true)
            }
            Key::Home | Key::End if state.open => {
                let indexes = enabled(disabled);
                let edge = if key == Key::Home {
                    indexes.first()
                } else {
                    indexes.last()
                };
                let active = edge.copied().unwrap_or(0);
                outcome(State { active, ..state }, None, true)
            }
            Key::Enter | Key::Space if state.open => {
                let chosen =
                    (!disabled.get(state.active).copied().unwrap_or(true)).then_some(state.active);
                // A disabled option leaves the listbox open, like shipping.
                let state = if chosen.is_some() { closed } else { state };
                outcome(state, chosen, true)
            }
            Key::Enter | Key::Space => outcome(open(disabled, selected), None, true),
            Key::Escape | Key::Home | Key::End => outcome(state, None, false),
        }
    }

    /// A rectangle in window coordinates (y down).
    #[derive(Clone, Copy, Debug, PartialEq, Serialize)]
    pub struct Rect {
        pub left: f64,
        pub top: f64,
        pub width: f64,
        pub height: f64,
    }

    /// Where the listbox goes: `max_height` caps its scrolling height and
    /// `min_width` matches the trigger.
    #[derive(Clone, Copy, Debug, PartialEq, Serialize)]
    pub struct Layout {
        pub above: bool,
        pub max_height: f64,
        pub top: f64,
        pub left: f64,
        pub min_width: f64,
        pub width: f64,
    }

    /// Shipping `placeCustomSelectMenu`: below the trigger unless it fits
    /// better above, right-aligned to the trigger and kept inside the window.
    pub fn layout(
        trigger: Rect,
        menu_width: f64,
        menu_height: f64,
        viewport_width: f64,
        viewport_height: f64,
        option_count: usize,
    ) -> Layout {
        let measured = if menu_height > 0. {
            menu_height
        } else {
            MAX_MENU_HEIGHT.min(option_count as f64 * 31. + 8.)
        };
        let desired = MAX_MENU_HEIGHT.min(measured);
        let bottom = trigger.top + trigger.height;
        let space_above = (trigger.top - VIEWPORT_PADDING).max(0.);
        let space_below = (viewport_height - bottom - VIEWPORT_PADDING).max(0.);
        let above = space_below < desired && space_above > space_below;
        let available = if above { space_above } else { space_below };
        let viewport_max = (viewport_height - VIEWPORT_PADDING * 2.).max(0.);
        let max_height = MAX_MENU_HEIGHT
            .min(if available > 0. {
                available
            } else {
                viewport_max
            })
            .min(viewport_max)
            .max(1.);
        let height = max_height.min(measured);
        let max_width = MAX_MENU_WIDTH.min((viewport_width - VIEWPORT_PADDING * 2.).max(0.));
        let width_cap = if max_width > 0. {
            max_width
        } else {
            trigger.width
        };
        let width = menu_width.max(trigger.width).min(width_cap);
        let max_left = viewport_width - VIEWPORT_PADDING - width;
        let left = (trigger.left + trigger.width - width)
            .max(VIEWPORT_PADDING)
            .min(max_left.max(VIEWPORT_PADDING));
        let top = if above {
            trigger.top - MENU_GAP - height
        } else {
            bottom + MENU_GAP
        };
        let max_top = VIEWPORT_PADDING.max(viewport_height - height - VIEWPORT_PADDING);
        Layout {
            above,
            max_height,
            top: top.max(VIEWPORT_PADDING).min(max_top),
            left,
            min_width: trigger.width.min(width_cap),
            width,
        }
    }
}

/// Shipping `NumberInput` stepper behaviour.
pub mod number {
    /// Decimal places of `step` as written (0.1 → 1, 1 → 0).
    pub fn decimal_places(step: f64) -> usize {
        let text = step.to_string();
        text.find('.').map_or(0, |dot| text.len() - dot - 1)
    }

    fn round_to(value: f64, places: usize) -> f64 {
        if places == 0 {
            value.round()
        } else {
            format!("{value:.places$}").parse().unwrap_or(value)
        }
    }

    /// Round to the step's precision and drop trailing zeros (1.50 → "1.5").
    pub fn format_stepped(value: f64, step: f64) -> String {
        let rounded = round_to(value, decimal_places(step));
        if rounded == 0. {
            return "0".into();
        }
        rounded.to_string()
    }

    fn clamp(value: f64, min: Option<f64>, max: Option<f64>) -> f64 {
        let value = min
            .filter(|m| m.is_finite())
            .map_or(value, |m| value.max(m));
        max.filter(|m| m.is_finite())
            .map_or(value, |m| value.min(m))
    }

    /// The value one step up (`up`) or down from `text`. An unparseable field
    /// steps from `min`, else zero; the result stays within the bounds.
    pub fn step_from(text: &str, step: f64, up: bool, min: Option<f64>, max: Option<f64>) -> f64 {
        let base = text
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .unwrap_or_else(|| min.filter(|m| m.is_finite()).unwrap_or(0.));
        let raw = base + if up { step } else { -step };
        clamp(round_to(raw, decimal_places(step)), min, max)
    }

    /// Whether the Decrease and Increase steppers are disabled at the bounds.
    pub fn at_bounds(text: &str, min: Option<f64>, max: Option<f64>) -> (bool, bool) {
        match text.trim().parse::<f64>().ok().filter(|v| v.is_finite()) {
            Some(value) => (
                min.is_some_and(|min| value <= min),
                max.is_some_and(|max| value >= max),
            ),
            None => (false, false),
        }
    }
}

/// Shipping `RangeSlider` track geometry.
pub mod range {
    /// Where `value` sits along the track, 0–1 (`--range-progress`, tick and
    /// label positions).
    pub fn fraction(value: f64, min: f64, max: f64) -> f64 {
        let span = (max - min).max(1.);
        ((value - min) / span).clamp(0., 1.)
    }

    /// The nearest `step` from `min` for a track fraction, within bounds.
    pub fn value_at(fraction: f64, min: f64, max: f64, step: f64) -> f64 {
        let raw = min + fraction.clamp(0., 1.) * (max - min);
        let stepped = if step > 0. {
            min + ((raw - min) / step).round() * step
        } else {
            raw
        };
        stepped.clamp(min.min(max), max.max(min))
    }
}

#[cfg(test)]
mod tests {
    use super::select::{Key, State};
    use super::*;

    #[test]
    fn select_keys_follow_shipping_custom_select() {
        let disabled = [false, true, false, false];
        let closed = State::default();
        let opened = select::key(closed, &disabled, 2, Key::ArrowDown);
        assert_eq!(
            opened.state,
            State {
                open: true,
                active: 2
            }
        );
        assert!(opened.handled);
        // Arrows skip disabled options and wrap.
        let down = select::key(opened.state, &disabled, 2, Key::ArrowDown);
        assert_eq!(down.state.active, 3);
        let wrap = select::key(down.state, &disabled, 2, Key::ArrowDown);
        assert_eq!(wrap.state.active, 0);
        let up = select::key(wrap.state, &disabled, 2, Key::ArrowUp);
        assert_eq!(up.state.active, 3);
        let up = select::key(
            select::State {
                open: true,
                active: 2,
            },
            &disabled,
            2,
            Key::ArrowUp,
        );
        assert_eq!(up.state.active, 0, "the disabled option is skipped");
        assert_eq!(
            select::key(up.state, &disabled, 2, Key::End).state.active,
            3
        );
        assert_eq!(
            select::key(up.state, &disabled, 2, Key::Home).state.active,
            0
        );
        let chosen = select::key(
            State {
                open: true,
                active: 3,
            },
            &disabled,
            2,
            Key::Enter,
        );
        assert_eq!(chosen.chosen, Some(3));
        assert!(!chosen.state.open);
        let escape = select::key(
            State {
                open: true,
                active: 3,
            },
            &disabled,
            2,
            Key::Escape,
        );
        assert!(!escape.state.open && escape.chosen.is_none() && escape.handled);
        // Closed: Escape, Home and End pass through; Space opens.
        for key in [Key::Escape, Key::Home, Key::End] {
            assert!(!select::key(closed, &disabled, 2, key).handled);
        }
        assert!(select::key(closed, &disabled, 2, Key::Space).state.open);
        // A disabled selection opens on the first enabled option.
        assert_eq!(select::open(&disabled, 1).active, 0);
    }

    #[test]
    fn select_menu_prefers_below_right_aligned_and_stays_inside() {
        let trigger = select::Rect {
            left: 300.,
            top: 100.,
            width: 120.,
            height: 32.,
        };
        let below = select::layout(trigger, 200., 150., 800., 600., 5);
        assert!(!below.above);
        assert_eq!(below.top, 138.);
        assert_eq!(below.left, 220., "right edge follows the trigger");
        assert_eq!(below.width, 200.);
        assert_eq!(below.min_width, 120.);
        let low = select::Rect {
            top: 540.,
            ..trigger
        };
        let above = select::layout(low, 100., 150., 800., 600., 5);
        assert!(above.above);
        assert_eq!(above.top, 540. - 6. - 150.);
        assert_eq!(above.width, 120., "at least as wide as the trigger");
        let edge = select::Rect {
            left: 0.,
            ..trigger
        };
        assert_eq!(select::layout(edge, 400., 100., 800., 600., 3).left, 8.);
        assert_eq!(select::layout(edge, 400., 100., 800., 600., 3).width, 360.);
        // Unmeasured menus estimate 31 pt rows.
        let estimate = select::layout(trigger, 0., 0., 800., 600., 20);
        assert_eq!(estimate.max_height, 240.);
    }

    #[test]
    fn number_steps_like_shipping_number_input() {
        assert_eq!(number::decimal_places(1.), 0);
        assert_eq!(number::decimal_places(0.1), 1);
        assert_eq!(number::decimal_places(0.25), 2);
        assert_eq!(number::step_from("5", 1., true, Some(0.), Some(10.)), 6.);
        assert_eq!(number::step_from("10", 1., true, Some(0.), Some(10.)), 10.);
        assert_eq!(number::step_from("0.2", 0.1, true, None, None), 0.3);
        assert_eq!(number::step_from("", 1., true, Some(2.), None), 3.);
        assert_eq!(number::step_from("abc", 1., false, None, None), -1.);
        assert_eq!(number::format_stepped(1.50, 0.1), "1.5");
        assert_eq!(number::format_stepped(2.0, 0.1), "2");
        assert_eq!(number::format_stepped(7.6, 1.), "8");
        assert_eq!(number::at_bounds("0", Some(0.), Some(10.)), (true, false));
        assert_eq!(number::at_bounds("10", Some(0.), Some(10.)), (false, true));
        assert_eq!(number::at_bounds("x", Some(0.), Some(10.)), (false, false));
    }

    #[test]
    fn range_positions_match_shipping_range_slider() {
        assert_eq!(range::fraction(100., 0., 200.), 0.5);
        assert_eq!(range::fraction(-5., 0., 200.), 0.);
        assert_eq!(range::fraction(3., 0., 0.), 1., "a zero span still paints");
        assert_eq!(range::value_at(0.5, 0., 200., 1.), 100.);
        assert_eq!(range::value_at(0.333, 0., 3., 1.), 1.);
        assert_eq!(range::value_at(2., 0., 3., 1.), 3.);
    }
}
