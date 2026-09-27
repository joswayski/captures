//! The live Preferences window: a separate, resizable top-level window beside
//! the Capture History root, as in the shipping app. It is an immediate child
//! viewport so it can borrow the host's settings state directly.
use captures_app::app_windows::{self, WindowSpec};
use eframe::egui;

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
    /// Declared hidden while a capture hides the workspace.
    hidden: bool,
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

    /// Declare the window for this root pass. `hidden` keeps it (and its
    /// state) while a capture hides the workspace.
    pub(crate) fn show<R>(
        &mut self,
        ctx: &egui::Context,
        hidden: bool,
        add: impl FnOnce(&mut egui::Ui) -> R,
    ) -> Option<Shown<R>> {
        if !self.open {
            return None;
        }
        self.hidden = hidden;
        let mut add = Some(add);
        let mut focused = self.focused;
        let shown = ctx.show_viewport_immediate(
            viewport(),
            builder(app_windows::PREFERENCES).with_visible(!hidden),
            |ui, _| {
                if ui.input(|input| input.viewport().close_requested()) {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    return Shown::Closed;
                }
                focused = ui.input(|input| input.viewport().focused.unwrap_or(false));
                Shown::Content(add.take().map(|add| add(ui)))
            },
        );
        self.focused = focused;
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
        ctx.run_ui(Default::default(), |ui| {
            assert!(matches!(
                window.show(ui.ctx(), false, |_| runs += 1),
                Some(Shown::Content(()))
            ));
        })
        .textures_delta
        .clear();
        assert_eq!(runs, 1);
    }
}
