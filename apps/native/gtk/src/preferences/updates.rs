//! Preferences' Updates card, as shipping's `UpdatePreferences`
//! (`apps/desktop/ui/src/App.tsx`): build identity beside one action button
//! with its status under it, the install progress, the open-captures warning,
//! the error and the captur.es fallback. The resident app owns the Preview
//! worker and the card's state (`crate::updates`); this only draws it.
//! Development builds keep the static "Updates unavailable" copy and never
//! make a request.
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use captures_app::preferences;
use gtk::glib;
use gtk::prelude::*;

use crate::updates::{Card, Step};

/// Shipping's Updates header, once the card can update (a Preview build).
pub const HEADER: &str =
    "Preview builds check for a new version automatically. Update now installs it in place.";

thread_local! {
    /// The open card, refreshed by the resident app.
    static OPEN: RefCell<Weak<Updates>> = const { RefCell::new(Weak::new()) };
}

pub struct Widgets {
    pub version: gtk::Label,
    pub channel: gtk::Label,
    pub button: gtk::Button,
    /// `.update-settings-status` (`role="status"`): heading and `small`.
    pub heading: gtk::Label,
    pub detail: gtk::Label,
    pub progress: gtk::ProgressBar,
    pub warning: gtk::Label,
    pub error: gtk::Label,
    /// The download fallback paragraph, shown with a Preview card.
    pub fallback: gtk::Widget,
}

pub struct Updates {
    widgets: Widgets,
    step: Cell<Option<Step>>,
    /// The pulse while the progress is indeterminate.
    pulse: RefCell<Option<gtk::TickCallbackId>>,
}

/// Keep the card's widgets in step with the resident app's update state.
pub fn connect(widgets: Widgets) -> Rc<Updates> {
    let updates = Rc::new(Updates {
        widgets,
        step: Cell::new(None),
        pulse: RefCell::default(),
    });
    OPEN.with(|open| *open.borrow_mut() = Rc::downgrade(&updates));
    let weak = Rc::downgrade(&updates);
    updates.widgets.button.connect_clicked(move |_| {
        if let Some(step) = weak.upgrade().and_then(|updates| updates.step.get()) {
            // The resident app refreshes the card; never under this borrow.
            glib::idle_add_local_once(move || crate::updates::act(step));
        }
    });
    updates.refresh();
    updates
}

/// The resident app's state changed.
pub fn refresh_open() {
    if let Some(updates) = OPEN.with(|open| open.borrow().upgrade()) {
        updates.refresh();
    }
}

impl Updates {
    fn refresh(&self) {
        let w = &self.widgets;
        let Some(card) = crate::updates::current() else {
            w.version.set_text(preferences::updates_version());
            w.channel.set_text(preferences::UPDATES_CHANNEL);
            set_button(&w.button, preferences::UPDATES_ACTION, false);
            describe(&w.button, preferences::UPDATES_DETAIL);
            w.heading.set_text(preferences::UPDATES_TITLE);
            w.detail.set_visible(false);
            for hidden in [w.warning.upcast_ref::<gtk::Widget>(), w.error.upcast_ref()] {
                hidden.set_visible(false);
            }
            w.fallback.set_visible(false);
            self.set_progress(None);
            self.step.set(None);
            return;
        };
        let Card {
            version,
            channel,
            action,
            step,
            heading,
            detail,
            current,
            progress,
            warning,
            error,
        } = card;
        w.version.set_text(&version);
        w.channel.set_text(channel);
        set_button(&w.button, action, step.is_some());
        describe(
            &w.button,
            if step == Some(Step::ViewRelease) {
                crate::updates::MANUAL_DETAIL
            } else {
                ""
            },
        );
        // An empty status keeps its line, as shipping's `\u00a0`.
        w.heading.set_text(if heading.is_empty() {
            "\u{a0}"
        } else {
            &heading
        });
        if current {
            w.heading.add_css_class("current");
        } else {
            w.heading.remove_css_class("current");
        }
        w.detail.set_text(&detail);
        w.detail.set_visible(!detail.is_empty());
        w.warning.set_text(warning.unwrap_or_default());
        w.warning.set_visible(warning.is_some());
        w.error.set_text(error.as_deref().unwrap_or_default());
        w.error.set_visible(error.is_some());
        w.fallback.set_visible(true);
        self.set_progress(progress);
        self.step.set(step);
    }

    /// `.update-settings-progress`: a fraction, or a pulse for an unknown
    /// total (`Some(None)`).
    fn set_progress(&self, progress: Option<Option<f32>>) {
        let bar = &self.widgets.progress;
        bar.set_visible(progress.is_some());
        let pulsing = matches!(progress, Some(None));
        if !pulsing && let Some(id) = self.pulse.borrow_mut().take() {
            id.remove();
        }
        match progress {
            Some(Some(fraction)) => {
                bar.set_fraction(f64::from(fraction));
                let percent = format!("{}%", (fraction * 100.).round());
                bar.update_property(&[
                    gtk::accessible::Property::ValueNow(f64::from(fraction) * 100.),
                    gtk::accessible::Property::ValueText(&percent),
                ]);
            }
            Some(None) if self.pulse.borrow().is_none() => {
                bar.update_property(&[gtk::accessible::Property::ValueText("Installing")]);
                *self.pulse.borrow_mut() = Some(bar.add_tick_callback(|bar, _| {
                    bar.pulse();
                    glib::ControlFlow::Continue
                }));
            }
            _ => {}
        }
    }
}

/// The action button's text and name; disabled while busy.
fn set_button(button: &gtk::Button, text: &str, enabled: bool) {
    if let Some(label) = button.child().and_downcast::<gtk::Label>() {
        label.set_text(text);
    }
    button.update_property(&[gtk::accessible::Property::Label(text)]);
    button.set_sensitive(enabled);
}

fn describe(button: &gtk::Button, detail: &str) {
    button.set_tooltip_text((!detail.is_empty()).then_some(detail));
    button.update_property(&[gtk::accessible::Property::Description(detail)]);
}
