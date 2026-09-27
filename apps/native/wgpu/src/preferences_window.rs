//! The live Preferences window: a separate, resizable top-level window beside
//! the Capture History root, as in the shipping app. It is an immediate child
//! viewport so it can borrow the host's settings state directly.
use std::cell::Cell;

use captures_app::app_windows::{self, WindowSpec};
use eframe::egui;

thread_local! {
    static IN_WINDOW_EVENT: Cell<bool> = const { Cell::new(false) };
    static ROOT_PASS_REQUESTED: Cell<bool> = const { Cell::new(false) };
}

/// Run `f` while the host dispatches a winit window event to eframe.
///
/// eframe creates an immediate viewport's native window only while it holds
/// the active event loop, which it provides for window events. A hidden
/// root is repainted outside them, so a window created then would fail.
pub(crate) fn during_window_event<R>(f: impl FnOnce() -> R) -> R {
    let previous = IN_WINDOW_EVENT.with(|cell| cell.replace(true));
    let result = f();
    IN_WINDOW_EVENT.with(|cell| cell.set(previous));
    result
}

/// Whether a root pass must be dispatched as a window event so the
/// Preferences window can be created. Clears the request.
pub(crate) fn take_root_pass_request() -> bool {
    ROOT_PASS_REQUESTED.with(|cell| cell.replace(false))
}

pub(crate) fn viewport() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("captures-preferences-window")
}

/// Builder for a document window with the shipping title and sizes.
pub(crate) fn builder(spec: WindowSpec) -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_title(spec.title)
        .with_inner_size(spec.size())
        .with_min_inner_size(spec.min_size())
        .with_resizable(true)
}

/// What one pass of the window produced.
pub(crate) enum Shown<R> {
    /// The user closed the window; it is no longer declared.
    Closed,
    Content(R),
}

#[derive(Default)]
pub(crate) struct PreferencesWindow {
    open: bool,
    focused: bool,
    /// Hidden while a capture hides the workspace.
    hidden: bool,
    /// The window's last on-screen frame (outer position, inner size).
    frame: Option<(egui::Pos2, egui::Vec2)>,
    /// Where a window hidden for a capture comes back.
    restore: Option<(egui::Pos2, egui::Vec2)>,
    /// Declared in the previous pass, so its native window exists.
    declared: bool,
}

impl PreferencesWindow {
    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    /// Open and showing its content (not hidden for a capture).
    pub(crate) fn presented(&self) -> bool {
        self.open && !self.hidden
    }

    pub(crate) fn focused(&self) -> bool {
        self.presented() && self.focused
    }

    /// Shipping `show_preferences`: create the window, or show, restore and
    /// focus the one that is already open.
    pub(crate) fn open(&mut self, ctx: &egui::Context) {
        if self.open {
            if !self.hidden {
                ctx.send_viewport_cmd_to(viewport(), egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd_to(viewport(), egui::ViewportCommand::Focus);
            }
        } else {
            self.open = true;
        }
        // The root may be hidden (closed History); paint it once so this
        // child window is declared.
        crate::live::request_hidden_root_paint(ctx);
        ctx.request_repaint();
    }

    pub(crate) fn close(&mut self) {
        *self = Self::default();
    }

    /// Declare the window for this root pass. While a capture hides the
    /// workspace (`hidden`) it is not declared: eframe paints every declared
    /// immediate viewport, and presenting to an unmapped window can stall the
    /// capture. It returns where it was, without taking focus.
    pub(crate) fn show<R>(
        &mut self,
        ctx: &egui::Context,
        hidden: bool,
        add: impl FnOnce(&mut egui::Ui) -> R,
    ) -> Option<Shown<R>> {
        if !self.open {
            return None;
        }
        if hidden {
            if !self.hidden {
                self.restore = self.frame;
            }
            self.hidden = true;
            self.focused = false;
            self.declared = false;
            return None;
        }
        self.hidden = false;
        if !self.declared && !IN_WINDOW_EVENT.with(Cell::get) {
            // Create the window in a root pass the host dispatches as a
            // window event (see `during_window_event`).
            ROOT_PASS_REQUESTED.with(|cell| cell.set(true));
            crate::live::request_hidden_root_paint(ctx);
            ctx.request_repaint();
            return None;
        }
        let mut builder = builder(app_windows::PREFERENCES);
        if let Some((position, size)) = self.restore {
            builder = builder.with_position(position).with_inner_size(size);
        }
        let mut add = Some(add);
        let mut focused = self.focused;
        let mut frame = self.frame;
        let shown = ctx.show_viewport_immediate(viewport(), builder, |ui, _| {
            if ui.input(|input| input.viewport().close_requested()) {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::CancelClose);
                return Shown::Closed;
            }
            ui.input(|input| {
                let info = input.viewport();
                focused = info.focused.unwrap_or(false);
                if let (Some(outer), Some(inner)) = (info.outer_rect, info.inner_rect) {
                    frame = Some((outer.min, inner.size()));
                }
            });
            Shown::Content(add.take().map(|add| add(ui)))
        });
        self.focused = focused;
        self.frame = frame;
        self.declared = true;
        match shown {
            Shown::Closed => {
                self.close();
                Some(Shown::Closed)
            }
            Shown::Content(content) => content.map(Shown::Content),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_uses_the_shipping_preferences_window() {
        let builder = builder(app_windows::PREFERENCES);
        assert_eq!(builder.title.as_deref(), Some("Captures Preferences"));
        assert_eq!(builder.inner_size, Some(egui::vec2(880., 660.)));
        assert_eq!(builder.min_inner_size, Some(egui::vec2(560., 440.)));
        assert_eq!(builder.resizable, Some(true));
    }

    #[test]
    fn a_capture_hides_the_window_and_restores_it_in_place() {
        let ctx = egui::Context::default();
        let mut window = PreferencesWindow::default();
        window.open(&ctx);
        window.frame = Some((egui::pos2(40., 60.), egui::vec2(700., 500.)));
        let mut runs = 0;
        ctx.run_ui(Default::default(), |ui| {
            assert!(window.show(ui.ctx(), true, |_| runs += 1).is_none());
        })
        .textures_delta
        .clear();
        assert_eq!(runs, 0, "a hidden window is not declared or painted");
        assert!(window.is_open() && !window.presented());
        assert_eq!(
            window.restore,
            Some((egui::pos2(40., 60.), egui::vec2(700., 500.)))
        );
        during_window_event(|| {
            ctx.run_ui(Default::default(), |ui| {
                assert!(window.show(ui.ctx(), false, |_| runs += 1).is_some());
            })
            .textures_delta
            .clear();
        });
        assert_eq!(runs, 1);
        assert!(window.presented());
        window.close();
        assert_eq!(window.restore, None, "closing forgets the placement");
    }

    #[test]
    fn opening_twice_focuses_and_closing_forgets_state() {
        let ctx = egui::Context::default();
        let mut window = PreferencesWindow::default();
        assert!(!window.is_open() && !window.presented());
        window.open(&ctx);
        assert!(window.is_open() && window.presented());
        window.open(&ctx);
        assert!(window.is_open());
        window.hidden = true;
        assert!(window.is_open() && !window.presented() && !window.focused());
        window.close();
        assert!(!window.is_open() && !window.presented());
    }

    #[test]
    fn show_declares_the_window_only_while_open() {
        // egui embeds immediate viewports without a native integration, so
        // this runs the content in the root pass.
        let ctx = egui::Context::default();
        let mut window = PreferencesWindow::default();
        let mut runs = 0;
        ctx.run_ui(Default::default(), |ui| {
            assert!(window.show(ui.ctx(), false, |_| runs += 1).is_none());
        })
        .textures_delta
        .clear();
        assert_eq!(runs, 0);
        window.open(&ctx);
        // Outside a window event the first declaration waits for a root pass
        // the host dispatches as one, where eframe can create the window.
        let _ = take_root_pass_request();
        ctx.run_ui(Default::default(), |ui| {
            assert!(window.show(ui.ctx(), false, |_| runs += 1).is_none());
        })
        .textures_delta
        .clear();
        assert_eq!(runs, 0);
        assert!(take_root_pass_request());
        during_window_event(|| {
            ctx.run_ui(Default::default(), |ui| {
                assert!(matches!(
                    window.show(ui.ctx(), false, |_| runs += 1),
                    Some(Shown::Content(()))
                ));
            })
            .textures_delta
            .clear();
        });
        assert_eq!(runs, 1);
        // Once created it is declared in any pass, window event or not.
        ctx.run_ui(Default::default(), |ui| {
            assert!(window.show(ui.ctx(), false, |_| runs += 1).is_some());
        })
        .textures_delta
        .clear();
        assert_eq!(runs, 2);
        assert!(!take_root_pass_request());
    }
}
