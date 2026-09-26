//! Behaviour of the shipping UI primitives (`CustomSelect.tsx`,
//! `lib/customSelectMenu.ts`) for native hosts: select keyboard
//! navigation and menu placement. Hosts only draw.

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
}
