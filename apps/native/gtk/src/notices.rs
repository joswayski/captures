//! Shipping's tray notices as small transparent windows in the tray corner:
//! the launch notice ("Captures is ready to use", `StartupNotice` and its
//! `.startup-notice*` rules) and the update notice (`UpdateNotice`,
//! `.update-*`). Copy, state and placement come from
//! `captures_app::tray_notice` and `captures_app::update_notice`.
//!
//! Linux StatusNotifier hosts never report the tray icon's position
//! (`tray_notice::should_retry_tray_rect`), so both notices take the shared
//! caretless fallback: the tray corner of the work area, with the launch
//! notice as the compact left-aligned rectangle. X11 positions the window
//! itself; Wayland uses wlr-layer-shell when the `layer-shell` feature and
//! compositor have it, and otherwise leaves placement to the compositor.

use std::cell::{Cell, RefCell};
use std::f64::consts::TAU;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use captures_app::tray_notice::{self, TRAY_NOTICE_FRAME_PAD};
use captures_app::update_notice::{
    self as notice, Action, Presentation, StubEvent, UpdateStatus, ViewState,
};
use gtk::prelude::*;
use gtk::{glib, graphene};

use crate::preferences::{description, label};
use crate::widgets::{Icon, MaxWidth, SnapshotArea};

pub const UPDATE_TITLE: &str = "Captures Update";

/// An undecorated, transparent notice window with the shared frame pad
/// around `card` for its shadow. The card is `width` wide: the window sizes
/// to its content, and wrapped copy would otherwise ask for its full line.
fn window(
    app: &gtk::Application,
    title: &str,
    card: &impl IsA<gtk::Widget>,
    width: f64,
) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(title)
        .decorated(false)
        .resizable(false)
        .build();
    window.add_css_class("captures");
    window.add_css_class("notice-window");
    let pad = TRAY_NOTICE_FRAME_PAD as i32;
    let frame = MaxWidth::new(card, width as i32);
    frame.set_margin_top(pad);
    frame.set_margin_bottom(pad);
    frame.set_margin_start(pad);
    frame.set_margin_end(pad);
    frame.set_halign(gtk::Align::Start);
    frame.set_valign(gtk::Align::Start);
    window.set_child(Some(&frame));
    window
}

/// Shipping's click-through notice, centered where the recording HUD was.
/// It borrows the notice frame, but not the tray-corner placement or focus.
pub fn recording_hidden(
    app: &gtk::Application,
    shortcut: &str,
    hud_window: &gtk::ApplicationWindow,
) -> gtk::ApplicationWindow {
    use captures_app::recording_hud as hud;
    let copy = hud::hidden_notice_copy(shortcut, captures_app::shortcuts::ShortcutPlatform::Linux);
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .accessible_role(gtk::AccessibleRole::Status)
        .build();
    card.add_css_class("recording-hidden-notice");
    card.set_size_request(
        hud::HIDDEN_NOTICE_WIDTH as i32,
        hud::HIDDEN_NOTICE_HEIGHT as i32,
    );
    card.update_property(&[
        gtk::accessible::Property::Label(copy.title),
        gtk::accessible::Property::Description(&copy.detail()),
    ]);
    if card.settings().is_gtk_enable_animations() {
        card.add_css_class("animated");
    }
    let tile = gtk::Box::new(gtk::Orientation::Vertical, 0);
    tile.add_css_class("recording-hidden-icon");
    tile.set_valign(gtk::Align::Center);
    tile.set_accessible_role(gtk::AccessibleRole::Presentation);
    let icon = Icon::new("capture", 20).with_stroke(1.8);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon.set_vexpand(true);
    tile.append(&icon);
    card.append(&tile);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 3);
    text.set_valign(gtk::Align::Center);
    text.set_hexpand(true);
    text.append(&label(copy.title, "recording-hidden-title"));
    let sentence: HiddenSentence = glib::Object::new();
    for (value, class) in std::iter::once((copy.before_keys.as_str(), "recording-hidden-text"))
        .chain(
            copy.keys
                .iter()
                .map(|key| (key.as_str(), "recording-hidden-key")),
        )
        .chain(std::iter::once((copy.after_keys, "recording-hidden-text")))
    {
        let item = label(value, class);
        item.set_halign(gtk::Align::Start);
        item.set_valign(gtk::Align::Center);
        item.set_parent(&sentence);
    }
    text.append(&sentence);
    card.append(&text);
    let window = window(app, copy.title, &card, hud::HIDDEN_NOTICE_WIDTH);
    window.set_focusable(false);
    window.connect_realize(|window| {
        if let Some(surface) = window.surface() {
            surface.set_input_region(&gtk::cairo::Region::create());
        }
    });
    #[cfg(feature = "x11")]
    if let Some(placer) = crate::previews::x11::Placer::new(&WidgetExt::display(hud_window)) {
        let anchor = hud_window
            .surface()
            .and_downcast::<gdk4_x11::X11Surface>()
            .and_then(|surface| placer.geometry(&surface));
        window.connect_realize(move |window| {
            if let Some(surface) = window.surface().and_downcast::<gdk4_x11::X11Surface>() {
                surface.set_skip_taskbar_hint(true);
                surface.set_skip_pager_hint(true);
                placer.unmanaged_notification(&surface);
                let (_, size) = window.preferred_size();
                if let Some(anchor) = anchor {
                    placer.move_to(
                        &surface,
                        anchor.x + (anchor.width - f64::from(size.width())) / 2.,
                        anchor.y + (anchor.height - f64::from(size.height())) / 2.,
                    );
                }
            }
        });
    }
    #[cfg(not(feature = "x11"))]
    let _ = hud_window;
    let weak = window.downgrade();
    glib::timeout_add_local_once(Duration::from_millis(6200), move || {
        if let Some(window) = weak.upgrade() {
            window.close();
        }
    });
    window
}

// Shipping uses flex-wrap for text runs and individual keycaps. GtkFlowBox
// aligns columns across rows, which puts multi-key shortcuts on separate lines.
mod sentence_imp {
    use gtk::glib;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;

    #[derive(Default)]
    pub struct HiddenSentence;

    #[glib::object_subclass]
    impl ObjectSubclass for HiddenSentence {
        const NAME: &'static str = "CapturesHiddenSentence";
        type Type = super::HiddenSentence;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_accessible_role(gtk::AccessibleRole::Group);
        }
    }

    impl ObjectImpl for HiddenSentence {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for HiddenSentence {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, width: i32) -> (i32, i32, i32, i32) {
            if orientation == gtk::Orientation::Horizontal {
                let mut min = 0;
                let mut natural = 0;
                let mut child = self.obj().first_child();
                while let Some(item) = child {
                    let (item_min, item_width, _, _) = item.measure(orientation, -1);
                    min = min.max(item_min);
                    natural += item_width + if natural == 0 { 0 } else { 4 };
                    child = item.next_sibling();
                }
                (min, natural, -1, -1)
            } else {
                let height = self.obj().arrange(width, false);
                (height, height, -1, -1)
            }
        }

        fn size_allocate(&self, width: i32, _height: i32, _baseline: i32) {
            self.obj().arrange(width, true);
        }
    }
}

glib::wrapper! {
    pub struct HiddenSentence(ObjectSubclass<sentence_imp::HiddenSentence>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl HiddenSentence {
    fn arrange(&self, width: i32, allocate: bool) -> i32 {
        let mut row: Vec<(gtk::Widget, i32, i32, i32)> = Vec::new();
        let (mut x, mut y, mut height) = (0, 0, 0);
        let flush = |row: &mut Vec<(gtk::Widget, i32, i32, i32)>, y, height| {
            for (item, x, w, h) in row.drain(..) {
                if allocate {
                    let at = gtk::gsk::Transform::new().translate(&graphene::Point::new(
                        x as f32,
                        (y + (height - h) / 2) as f32,
                    ));
                    item.allocate(w, h, -1, Some(at));
                }
            }
        };
        let mut child = self.first_child();
        while let Some(item) = child {
            let w = item.measure(gtk::Orientation::Horizontal, -1).1;
            let h = item.measure(gtk::Orientation::Vertical, w).1;
            if width >= 0 && x > 0 && x + w > width {
                flush(&mut row, y, height);
                y += height + 4;
                x = 0;
                height = 0;
            }
            child = item.next_sibling();
            row.push((item, x, w, h));
            x += w + 4;
            height = height.max(h);
        }
        flush(&mut row, y, height);
        y + height
    }
}

/// Put `window` in the tray corner: shipping's `fallback_tray_notice`
/// (work-area right edge, the panel's edge). `unmanaged` makes it a
/// notification the window manager never focuses or moves (the launch
/// notice); otherwise it stays a normal window kept above every workspace.
fn place(window: &gtk::ApplicationWindow, unmanaged: bool) {
    // Wayland has no work area to read; the layer surface's margins count
    // from the usable area, as the X11 fallback does from the work area.
    #[cfg(feature = "layer-shell")]
    let layered = crate::previews::layer_shell::notice(
        window,
        true,
        (18, tray_notice::TRAY_NOTICE_SCREEN_MARGIN as i32),
    );
    #[cfg(not(feature = "layer-shell"))]
    let layered = false;
    #[cfg(feature = "x11")]
    if !layered {
        place_on_x11(window, unmanaged);
    }
    #[cfg(not(feature = "x11"))]
    let _ = (window, layered, unmanaged);
}

/// The X11 half of [`place`]; a no-op on other displays.
#[cfg(feature = "x11")]
fn place_on_x11(window: &gtk::ApplicationWindow, unmanaged: bool) {
    use crate::previews::x11::Placer;
    use gdk4_x11::X11Surface;
    let Some(placer) = Placer::new(&WidgetExt::display(window)).map(Rc::new) else {
        return;
    };
    window.connect_realize(move |window| {
        let Some(surface) = window.surface().and_downcast::<X11Surface>() else {
            return;
        };
        surface.set_skip_taskbar_hint(true);
        surface.set_skip_pager_hint(true);
        if unmanaged {
            placer.unmanaged_notification(&surface);
        }
        let corner = {
            let (placer, window) = (placer.clone(), window.downgrade());
            Rc::new(move |surface: &X11Surface| {
                let (Some(window), Some((monitor, work))) =
                    (window.upgrade(), placer.areas(surface))
                else {
                    return;
                };
                // The window sizes to its content; the card is that
                // minus the frame pad.
                let (_, natural) = window.preferred_size();
                let pad = TRAY_NOTICE_FRAME_PAD * 2.;
                let placement = tray_notice::fallback_tray_notice(
                    monitor,
                    work,
                    tray_notice::FallbackEdge::WorkAreaInsets.resolve(monitor, work),
                    f64::from(natural.width()) - pad,
                    f64::from(natural.height()) - pad,
                );
                placer.move_to(surface, placement.x, placement.y);
            })
        };
        corner(&surface);
        // A bottom panel keeps the card's bottom edge as it grows.
        let resized = corner.clone();
        surface.connect_height_notify(move |surface| resized(surface));
        if !unmanaged {
            let (pinned, placer_ref) = (surface.downgrade(), Rc::downgrade(&placer));
            placer.watch_managed(&surface, move || {
                if let (Some(surface), Some(placer)) = (pinned.upgrade(), placer_ref.upgrade()) {
                    placer.pin(&surface);
                    // The window manager may have placed it itself.
                    corner(&surface);
                }
            });
        }
    });
}

fn button(text: &str, classes: &[&str]) -> gtk::Button {
    // A label child, not `with_label`: GTK 4.14 names a label-mode button
    // after its text and drops the accessible label ("HIDE", "#265"). GTK
    // 4.8 does not name it from the child, so the text is the default name.
    let button = gtk::Button::new();
    button.set_child(Some(&gtk::Label::new(Some(text))));
    button.update_property(&[gtk::accessible::Property::Label(text)]);
    for class in classes {
        button.add_css_class(class);
    }
    button
}

// ------------------------------------------------------------ launch ----

/// The launch notice for `visible_for`, with the New Capture keys (or, with
/// none, the tray route). Closes itself; Close closes it early.
pub fn startup(
    app: &gtk::Application,
    keys: &[String],
    visible_for: Duration,
) -> gtk::ApplicationWindow {
    let card = gtk::Overlay::builder()
        .accessible_role(gtk::AccessibleRole::Status)
        .build();
    card.add_css_class("startup-notice");
    card.set_size_request(
        tray_notice::STARTUP_NOTICE_WIDTH as i32,
        tray_notice::STARTUP_NOTICE_HEIGHT as i32,
    );
    card.update_property(&[gtk::accessible::Property::Label(
        tray_notice::STARTUP_NOTICE_TITLE,
    )]);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.add_css_class("startup-notice-content");
    content.set_valign(gtk::Align::Center);
    content.append(&label(
        tray_notice::STARTUP_NOTICE_TITLE,
        "startup-notice-title",
    ));
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    row.add_css_class("startup-notice-row");
    row.append(&label(
        if keys.is_empty() {
            "Open History from the tray menu"
        } else {
            tray_notice::STARTUP_NOTICE_HINT
        },
        "startup-notice-hint",
    ));
    for (index, key) in keys.iter().enumerate() {
        let chip = label(key, "startup-notice-key");
        chip.set_valign(gtk::Align::Center);
        // Shipping's word space before the first `<kbd>`, then its margin.
        chip.add_css_class(if index == 0 { "first" } else { "next" });
        row.append(&chip);
    }
    content.append(&row);
    card.set_child(Some(&content));
    let close = gtk::Button::new();
    close.add_css_class("startup-notice-dismiss");
    close.set_child(Some(&Icon::new("close", 14).with_stroke(2.)));
    close.set_tooltip_text(None);
    close.update_property(&[gtk::accessible::Property::Label("Close")]);
    close.set_halign(gtk::Align::End);
    close.set_valign(gtk::Align::Center);
    card.add_overlay(&close);

    let window = window(
        app,
        tray_notice::STARTUP_NOTICE_WINDOW_TITLE,
        &card,
        tray_notice::STARTUP_NOTICE_WIDTH,
    );
    window.add_css_class("startup-notice-window");
    place(&window, true);
    let weak = window.downgrade();
    close.connect_clicked(move |_| {
        if let Some(window) = weak.upgrade() {
            crate::emit("startup-notice-closed", serde_json::json!({"by": "close"}));
            window.close();
        }
    });
    let weak = window.downgrade();
    glib::timeout_add_local_once(visible_for, move || {
        if let Some(window) = weak.upgrade() {
            crate::emit(
                "startup-notice-closed",
                serde_json::json!({"by": "timeout"}),
            );
            window.close();
        }
    });
    window
}

// ------------------------------------------------------------ update ----

/// Widgets a progress tick updates in place instead of rebuilding.
#[derive(Default)]
struct Live {
    /// The presentation without its progress, for spotting a real change.
    shape: String,
    detail: Option<gtk::Label>,
    percent: Option<gtk::Label>,
    bar: Option<gtk::ProgressBar>,
    restart: Option<gtk::Label>,
}

/// The update notice: either a shared fixture status (`--scene update`)
/// walked through `update_notice::stub_next`, which never downloads or
/// installs, or the live Preview status from `updates.rs`, whose actions
/// run the real check, download, install and links.
pub struct UpdateNotice {
    pub window: gtk::ApplicationWindow,
    card: gtk::Box,
    settings_file: PathBuf,
    /// Driven by `updates.rs` rather than the stub.
    live_source: bool,
    status: RefCell<Option<UpdateStatus>>,
    view: RefCell<ViewState>,
    live: RefCell<Live>,
    tick: RefCell<Option<glib::SourceId>>,
    restart_started: Cell<Option<i64>>,
}

pub fn update(app: &gtk::Application, settings_file: PathBuf, fixture: &str) -> Rc<UpdateNotice> {
    update_notice(app, settings_file, notice::fixture(fixture), false, false)
}

/// The notice on a live status. Like shipping's non-focusable notice
/// window, it opens without taking focus from the app in use (X11: a zero
/// user time); a click on it still works.
pub fn update_live(
    app: &gtk::Application,
    settings_file: PathBuf,
    status: UpdateStatus,
    installing: bool,
) -> Rc<UpdateNotice> {
    let notice = update_notice(app, settings_file, Some(status), installing, true);
    #[cfg(feature = "x11")]
    notice.window.connect_realize(|window| {
        if let Some(surface) = window.surface().and_downcast::<gdk4_x11::X11Surface>() {
            surface.set_user_time(0);
        }
    });
    notice
}

fn update_notice(
    app: &gtk::Application,
    settings_file: PathBuf,
    status: Option<UpdateStatus>,
    installing: bool,
    live_source: bool,
) -> Rc<UpdateNotice> {
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .accessible_role(gtk::AccessibleRole::Dialog)
        .focusable(true)
        .build();
    card.add_css_class("update-notice");
    card.set_overflow(gtk::Overflow::Hidden);
    let window = window(app, UPDATE_TITLE, &card, notice::CARD_WIDTH);
    place(&window, false);
    let show_changelog = captures_settings::load(&settings_file)
        .map(|settings| settings.show_update_changelog)
        .unwrap_or(true);
    let notice = Rc::new(UpdateNotice {
        window,
        card,
        settings_file,
        live_source,
        status: RefCell::new(status),
        view: RefCell::new(ViewState {
            show_changelog,
            installing,
            ..ViewState::default()
        }),
        live: RefCell::default(),
        tick: RefCell::default(),
        restart_started: Cell::default(),
    });
    // Escape dismisses unless a download or restart is running.
    let keys = gtk::EventControllerKey::new();
    let weak = Rc::downgrade(&notice);
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gtk::gdk::Key::Escape
            && let Some(notice) = weak.upgrade()
        {
            notice.act(Action::Dismiss);
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    notice.window.add_controller(keys);
    notice.render();
    notice
}

impl UpdateNotice {
    fn presentation(&self) -> Presentation {
        notice::present(self.status.borrow().as_ref(), &self.view.borrow())
    }

    /// A notice action, as `UpdateNotice`'s handlers and the wgpu fixture.
    fn act(self: &Rc<Self>, action: Action) {
        crate::emit("update-notice-action", serde_json::json!(action));
        match action {
            Action::Dismiss => {
                if self.presentation().dismiss_blocked {
                    crate::emit("update-notice-dismiss-blocked", serde_json::json!({}));
                    return;
                }
                self.close();
            }
            Action::ShowNotes | Action::HideNotes => {
                let show = action == Action::ShowNotes;
                self.view.borrow_mut().show_changelog = show;
                // Shipping persists the choice for the next notice.
                if let Ok(mut settings) = captures_settings::load(&self.settings_file) {
                    settings.show_update_changelog = show;
                    let _ = captures_settings::save(&self.settings_file, &settings);
                }
                self.render();
            }
            Action::Install if self.live_source => crate::updates::act_from_notice(),
            Action::Check if self.live_source => crate::updates::act(crate::updates::Step::Check),
            Action::OpenPullRequest { url } if self.live_source => {
                open_url("pull_request", &url);
            }
            Action::OpenDownloadPage if self.live_source => {
                open_url("download", notice::DOWNLOAD_PAGE_URL);
            }
            Action::Install | Action::Check => {
                let status = self.status.borrow().clone();
                let Some(status) = status else { return };
                if matches!(
                    status,
                    UpdateStatus::Available {
                        installable: false,
                        ..
                    }
                ) {
                    // View release: the fixture never opens a browser.
                    crate::emit(
                        "update-notice-open-url",
                        serde_json::json!({"kind": "release", "opened": false}),
                    );
                    return;
                }
                let event = if action == Action::Install {
                    StubEvent::Install
                } else {
                    StubEvent::Check
                };
                self.advance(notice::stub_next(&status, event));
            }
            Action::OpenPullRequest { url } => crate::emit(
                "update-notice-open-url",
                serde_json::json!({"kind": "pull_request", "url": url, "opened": false}),
            ),
            Action::OpenDownloadPage => crate::emit(
                "update-notice-open-url",
                serde_json::json!({"kind": "download", "opened": false}),
            ),
            Action::DownloadVerify | Action::CancelDownload => {}
        }
    }

    /// Move to `next` (`None` closes: the stub restart finished) and
    /// schedule the stub's next tick.
    fn advance(self: &Rc<Self>, next: Option<UpdateStatus>) {
        if let Some(source) = self.tick.borrow_mut().take() {
            source.remove();
        }
        let Some(next) = next else {
            self.close();
            return;
        };
        let interval = notice::stub_tick_interval_ms(&next);
        *self.status.borrow_mut() = Some(next);
        self.render();
        if let Some(interval) = interval {
            let weak = Rc::downgrade(self);
            *self.tick.borrow_mut() = Some(glib::timeout_add_local_once(
                Duration::from_millis(interval),
                move || {
                    if let Some(notice) = weak.upgrade() {
                        notice.tick.borrow_mut().take();
                        let status = notice.status.borrow().clone();
                        if let Some(status) = status {
                            notice.advance(notice::stub_next(&status, StubEvent::Tick));
                        }
                    }
                },
            ));
        }
    }

    fn close(&self) {
        if let Some(source) = self.tick.borrow_mut().take() {
            source.remove();
        }
        crate::emit("update-notice-dismissed", serde_json::json!({}));
        self.window.close();
    }

    /// Follow the live status in place: no raise, no focus.
    pub fn set_live(self: &Rc<Self>, status: UpdateStatus, installing: bool) {
        let unchanged = self.status.borrow().as_ref() == Some(&status)
            && self.view.borrow().installing == installing;
        if unchanged {
            return;
        }
        *self.status.borrow_mut() = Some(status);
        self.view.borrow_mut().installing = installing;
        self.render();
    }

    /// The app is quitting to install the update.
    pub fn close_for_install(&self) {
        crate::emit("update-notice-closed", serde_json::json!({"by": "install"}));
        self.window.close();
    }

    /// Rebuild the card for a new state, or only refresh its progress.
    fn render(self: &Rc<Self>) {
        let p = self.presentation();
        let mut shape = p.clone();
        shape.download = None;
        shape.restart = None;
        let shape = serde_json::to_string(&shape).unwrap_or_default();
        {
            let live = self.live.borrow();
            if live.shape == shape {
                if let (Some(download), Some(detail), Some(bar)) =
                    (&p.download, &live.detail, &live.bar)
                {
                    detail.set_text(&download.detail);
                    if let (Some(percent), Some(text)) = (&live.percent, &download.percent_label) {
                        percent.set_text(text);
                    }
                    set_progress(bar, download);
                }
                if let (Some(restart), Some(message)) = (&p.restart, &live.restart) {
                    message.set_text(&restart.message);
                }
                return;
            }
        }
        while let Some(child) = self.card.first_child() {
            self.card.remove(&child);
        }
        let mut live = Live {
            shape,
            ..Live::default()
        };
        self.build(&p, &mut live);
        *self.live.borrow_mut() = live;
        crate::emit(
            "update-notice",
            serde_json::json!({"state": p.visual_state, "title": p.title}),
        );
    }

    fn build(self: &Rc<Self>, p: &Presentation, live: &mut Live) {
        let card = &self.card;
        for class in card.css_classes() {
            if class.starts_with("update-notice-") {
                card.remove_css_class(&class);
            }
        }
        card.add_css_class(&format!("update-notice-{}", p.visual_state));
        card.set_size_request(p.card_width as i32, p.card_height as i32);
        let action = |action: Action| {
            let weak = Rc::downgrade(self);
            move |_: &gtk::Button| {
                if let Some(notice) = weak.upgrade() {
                    notice.act(action.clone());
                }
            }
        };

        // `.update-notice-header`: icon, title and version, What's new.
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        header.add_css_class("update-notice-header");
        let icon = gtk::Box::new(gtk::Orientation::Vertical, 0);
        icon.add_css_class("update-app-icon");
        icon.set_valign(gtk::Align::Center);
        let glyph: gtk::Widget = match p.icon {
            notice::Icon::Spinner => spinner(14).upcast(),
            notice::Icon::App => Icon::new("capture", 18).upcast(),
            notice::Icon::Check => Icon::new("check", 18).upcast(),
            notice::Icon::Warning => Icon::new("warning", 18).upcast(),
        };
        glyph.set_halign(gtk::Align::Center);
        glyph.set_valign(gtk::Align::Center);
        glyph.set_vexpand(true);
        icon.append(&glyph);
        header.append(&icon);
        let copy = gtk::Box::new(gtk::Orientation::Vertical, 0);
        copy.add_css_class("update-notice-copy");
        copy.set_hexpand(true);
        copy.set_valign(gtk::Align::Center);
        let title = label(&p.title, "update-notice-title");
        title.set_accessible_role(gtk::AccessibleRole::Heading);
        let subtitle = description(&p.description, "update-notice-description");
        copy.append(&title);
        copy.append(&subtitle);
        header.append(&copy);
        card.update_relation(&[gtk::accessible::Relation::DescribedBy(&[
            subtitle.upcast_ref()
        ])]);
        card.update_property(&[gtk::accessible::Property::Label(&p.title)]);
        if let Some(reveal) = &p.reveal_notes {
            let button = button(&reveal.label.to_uppercase(), &["update-notes-reveal"]);
            button.update_property(&[gtk::accessible::Property::Label(&reveal.label)]);
            button.set_valign(gtk::Align::Center);
            button.connect_clicked(action(reveal.action.clone()));
            header.append(&button);
        }

        // `.update-notice-body`: notes, progress, restart, error or status.
        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.add_css_class("update-notice-body");
        if let Some(notes) = &p.notes {
            let section = gtk::Box::new(gtk::Orientation::Vertical, 0);
            section.add_css_class("update-notes");
            section.set_vexpand(true);
            section.update_property(&[gtk::accessible::Property::Label("What's new")]);
            let heading = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            heading.add_css_class("update-notes-heading");
            let h2 = label(&notes.heading.to_uppercase(), "update-notes-title");
            h2.set_hexpand(true);
            h2.set_accessible_role(gtk::AccessibleRole::Heading);
            h2.update_property(&[gtk::accessible::Property::Label(&notes.heading)]);
            heading.append(&h2);
            let hide = button(&notes.hide.label.to_uppercase(), &["update-notes-toggle"]);
            hide.update_property(&[gtk::accessible::Property::Label(&notes.hide.label)]);
            hide.connect_clicked(action(notes.hide.action.clone()));
            heading.append(&hide);
            section.append(&heading);
            if let Some(intro) = &notes.intro {
                section.append(&description(intro, "update-notes-intro"));
            }
            let groups = gtk::Box::new(gtk::Orientation::Vertical, 0);
            groups.add_css_class("update-notes-groups");
            for group in &notes.groups {
                let block = gtk::Box::new(gtk::Orientation::Vertical, 0);
                block.add_css_class("update-notes-group");
                if notes.stacked {
                    let h3 = label(&group.display_version, "update-notes-version");
                    h3.set_accessible_role(gtk::AccessibleRole::Heading);
                    block.append(&h3);
                }
                let list = gtk::Box::builder()
                    .orientation(gtk::Orientation::Vertical)
                    .accessible_role(gtk::AccessibleRole::List)
                    .build();
                list.add_css_class("update-notes-list");
                list.update_property(&[gtk::accessible::Property::Label(&group.display_version)]);
                for item in &group.items {
                    list.append(&note_item(self, item));
                }
                if group.items.is_empty() {
                    block.append(&description(&notes.group_empty, "update-notes-empty"));
                } else {
                    block.append(&list);
                }
                groups.append(&block);
            }
            if let Some(empty) = &notes.empty {
                section.append(&description(empty, "update-notes-empty"));
            } else {
                // Long notes scroll inside the card's fixed height.
                let scroller = gtk::ScrolledWindow::builder()
                    .hscrollbar_policy(gtk::PolicyType::Never)
                    .propagate_natural_height(false)
                    .vexpand(true)
                    .child(&groups)
                    .build();
                scroller.add_css_class("update-notes-scroll");
                section.append(&scroller);
            }
            body.append(&section);
        }
        if let Some(download) = &p.download {
            let section = gtk::Box::new(gtk::Orientation::Vertical, 0);
            section.add_css_class("update-download");
            section.update_property(&[gtk::accessible::Property::Label(
                "Update installation progress",
            )]);
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            row.add_css_class("update-progress-label");
            row.append(&label(&download.label, "update-progress-name"));
            let detail = label(&download.detail, "update-progress-detail");
            detail.set_hexpand(true);
            row.append(&detail);
            if let Some(text) = &download.percent_label {
                let percent = label(text, "update-progress-percent");
                row.append(&percent);
                live.percent = Some(percent);
            }
            section.append(&row);
            let bar = gtk::ProgressBar::new();
            bar.add_css_class("update-progress");
            bar.update_property(&[gtk::accessible::Property::Label("Downloading update")]);
            set_progress(&bar, download);
            if download.percent.is_none() {
                bar.add_tick_callback(|bar, _| {
                    bar.pulse();
                    glib::ControlFlow::Continue
                });
            }
            section.append(&bar);
            body.append(&section);
            live.detail = Some(detail);
            live.bar = Some(bar);
        }
        if let Some(restart) = &p.restart {
            let section = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .accessible_role(gtk::AccessibleRole::Status)
                .build();
            section.add_css_class("update-restart");
            let message = description(&restart.message, "update-status-message");
            section.append(&message);
            // `update-restart-progress`: the bar empties over three seconds.
            let bar = gtk::ProgressBar::new();
            bar.add_css_class("update-progress");
            bar.set_accessible_role(gtk::AccessibleRole::Presentation);
            let started = self
                .restart_started
                .get()
                .unwrap_or_else(glib::monotonic_time);
            self.restart_started.set(Some(started));
            bar.set_fraction(1.);
            bar.add_tick_callback(move |bar, clock| {
                let elapsed = (clock.frame_time() - started) as f64 / 3_000_000.;
                bar.set_fraction((1. - elapsed).clamp(0., 1.));
                if elapsed >= 1. {
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            });
            section.append(&bar);
            body.append(&section);
            live.restart = Some(message);
        } else {
            self.restart_started.set(None);
        }
        if let Some(error) = &p.error {
            let message = description(&error.message, "update-error");
            message.set_accessible_role(gtk::AccessibleRole::Alert);
            body.append(&message);
            // "You can also download from captur.es." with the link inline.
            let weak = Rc::downgrade(self);
            let fallback = inline(
                &error.fallback_prefix,
                (!error.fallback_link.is_empty()).then(|| -> InlineButton {
                    (
                        &error.fallback_link,
                        &error.fallback_link,
                        Box::new(move || {
                            if let Some(notice) = weak.upgrade() {
                                notice.act(Action::OpenDownloadPage);
                            }
                        }),
                    )
                }),
                &error.fallback_suffix,
                "update-download-fallback",
            );
            fallback.add_css_class("update-download-fallback-row");
            body.append(&fallback);
        }
        if let Some(message) = &p.status_message {
            let status = description(message, "update-status-message");
            status.set_accessible_role(gtk::AccessibleRole::Status);
            body.append(&status);
        }
        let compact = p.notes.is_none();
        let mut rows: Vec<gtk::Widget> = vec![header.upcast()];
        // `.update-notice-compact .update-notice-body:empty { display: none }`.
        if body.first_child().is_some() {
            rows.push(body.clone().upcast());
        }
        if let Some(warning) = &p.close_warning {
            let row = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .accessible_role(gtk::AccessibleRole::Status)
                .build();
            row.add_css_class("update-close-warning");
            let icon = Icon::new("warning", 16);
            icon.add_css_class("update-close-warning-icon");
            icon.set_valign(gtk::Align::Start);
            row.append(&icon);
            let text = description(warning, "update-close-warning-text");
            text.set_hexpand(true);
            row.append(&text);
            rows.push(row.upcast());
        }
        if let Some(footer) = &p.footer {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            row.add_css_class("update-notice-footer");
            let dismiss = button(&footer.dismiss.label, &["update-dismiss"]);
            dismiss.set_sensitive(footer.dismiss.enabled);
            dismiss.set_valign(gtk::Align::Center);
            dismiss.connect_clicked(action(footer.dismiss.action.clone()));
            // `margin-right: auto` beside a primary; otherwise flush right.
            if footer.primary.is_some() {
                dismiss.set_hexpand(true);
                dismiss.set_halign(gtk::Align::Start);
            } else {
                row.set_halign(gtk::Align::Fill);
                dismiss.set_hexpand(true);
                dismiss.set_halign(gtk::Align::End);
            }
            row.append(&dismiss);
            if let Some(primary) = &footer.primary {
                let button = button(&primary.label, &["update-primary"]);
                button.set_sensitive(primary.enabled);
                button.set_valign(gtk::Align::Center);
                button.connect_clicked(action(primary.action.clone()));
                row.append(&button);
            }
            rows.push(row.upcast());
        }
        // Shipping's four grid rows: compact cards share the spare height
        // between every row track (empty ones last); with notes, the body
        // takes it all.
        while compact && rows.len() < 4 {
            rows.push(gtk::Box::new(gtk::Orientation::Vertical, 0).upcast());
        }
        for row in rows {
            row.set_vexpand(if compact {
                true
            } else {
                row == *body.upcast_ref::<gtk::Widget>()
            });
            card.append(&row);
        }
    }
}

/// A link from the live notice, in the default browser.
fn open_url(kind: &str, url: &str) {
    let opened = gtk::gio::AppInfo::launch_default_for_uri(url, gtk::gio::AppLaunchContext::NONE);
    if let Err(error) = &opened {
        eprintln!("Could not open {url}: {error}");
    }
    crate::emit(
        "update-notice-open-url",
        serde_json::json!({"kind": kind, "url": url, "opened": opened.is_ok()}),
    );
}

fn set_progress(bar: &gtk::ProgressBar, download: &notice::Download) {
    if let Some(percent) = download.percent {
        bar.set_fraction(f64::from(percent) / 100.);
    }
    bar.update_property(&[gtk::accessible::Property::ValueText(
        &download.accessible_value,
    )]);
}

/// A release note bullet with its pull request link inline, as shipping's
/// `<li>{text} <button>#123</button></li>`.
fn note_item(notice: &Rc<UpdateNotice>, item: &notice::NoteItem) -> gtk::Box {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .accessible_role(gtk::AccessibleRole::ListItem)
        .build();
    row.add_css_class("update-notes-item");
    // `li::before`: a 5 px dot in `--text-faint`.
    let bullet = gtk::Box::new(gtk::Orientation::Vertical, 0);
    bullet.add_css_class("update-notes-bullet");
    bullet.set_valign(gtk::Align::Start);
    row.append(&bullet);
    let text = match &item.pull_request {
        Some(pull) => {
            let (weak, url) = (Rc::downgrade(notice), pull.url.clone());
            inline(
                &item.text,
                Some((
                    &format!("#{}", pull.number),
                    &format!("Open pull request {}", pull.number),
                    Box::new(move || {
                        if let Some(notice) = weak.upgrade() {
                            notice.act(Action::OpenPullRequest { url: url.clone() });
                        }
                    }),
                )),
                "",
                "update-notes-text",
            )
        }
        None => inline(&item.text, None, "", "update-notes-text"),
    };
    row.append(&text);
    row
}

pub(crate) type InlineButton<'a> = (&'a str, &'a str, Box<dyn Fn()>);

/// Wrapping text with an optional text button after it (shipping's
/// `<p>…<button>…</button>…</p>`): the button follows the last line when it
/// fits and wraps to its own line otherwise, and stays a real keyboard and
/// AT-SPI button, which a label link is not.
pub(crate) fn inline(
    before: &str,
    button: Option<InlineButton>,
    after: &str,
    class: &str,
) -> gtk::Widget {
    let text = description(before.trim_end(), class);
    let Some((label, accessible, clicked)) = button else {
        text.set_text(&format!("{}{after}", before.trim_end()));
        return text.upcast();
    };
    let tail = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let link = self::button(label, &["notice-inline-button"]);
    link.update_property(&[gtk::accessible::Property::Label(accessible)]);
    link.connect_clicked(move |_| clicked());
    tail.append(&link);
    if !after.is_empty() {
        tail.append(&self::label(after, class));
    }
    InlineTail::new(&text, &tail).upcast()
}

mod tail_imp {
    use gtk::glib;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;

    #[derive(Default)]
    pub struct InlineTail;

    #[glib::object_subclass]
    impl ObjectSubclass for InlineTail {
        const NAME: &'static str = "CapturesInlineTail";
        type Type = super::InlineTail;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for InlineTail {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for InlineTail {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let widget = self.obj();
            let (text, tail) = widget.parts();
            if orientation == gtk::Orientation::Horizontal {
                let (text_min, text_natural, _, _) = text.measure(orientation, -1);
                let (tail_min, tail_natural, _, _) = tail.measure(orientation, -1);
                return (
                    text_min.max(tail_min),
                    text_natural + widget.space() + tail_natural,
                    -1,
                    -1,
                );
            }
            let width = if for_size < 0 {
                widget.measure(gtk::Orientation::Horizontal, -1).1
            } else {
                for_size
            };
            let height = widget.arrange(width).2;
            (height, height, -1, -1)
        }

        fn size_allocate(&self, width: i32, _height: i32, _baseline: i32) {
            let widget = self.obj();
            let (text, tail) = widget.parts();
            let (text_height, (x, y, tail_width, line), _) = widget.arrange(width);
            text.allocate(width, text_height, -1, None);
            let at = gtk::gsk::Transform::new()
                .translate(&gtk::graphene::Point::new(x as f32, y as f32));
            tail.allocate(tail_width, line, -1, Some(at));
        }
    }
}

glib::wrapper! {
    pub struct InlineTail(ObjectSubclass<tail_imp::InlineTail>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl InlineTail {
    fn new(text: &gtk::Label, tail: &impl IsA<gtk::Widget>) -> Self {
        let widget: Self = glib::Object::new();
        text.set_parent(&widget);
        tail.set_parent(&widget);
        widget
    }

    fn parts(&self) -> (gtk::Label, gtk::Widget) {
        let text = self
            .first_child()
            .and_downcast::<gtk::Label>()
            .expect("text");
        (text, self.last_child().expect("tail"))
    }

    /// The word space before the tail, in the text's font.
    fn space(&self) -> i32 {
        let text = self.parts().0;
        let with = text.create_pango_layout(Some("a a")).pixel_size().0;
        let without = text.create_pango_layout(Some("aa")).pixel_size().0;
        with - without
    }

    /// For `width`: the text's height, the tail's `(x, y, width, line)` and
    /// the total height. Lines share one height, so the last line starts at
    /// the text height less one line.
    fn arrange(&self, width: i32) -> (i32, (i32, i32, i32, i32), i32) {
        let (text, tail) = self.parts();
        let text_height = text.measure(gtk::Orientation::Vertical, width).1;
        let tail_width = tail.measure(gtk::Orientation::Horizontal, -1).1.min(width);
        let layout = text.create_pango_layout(Some(&text.text()));
        layout.set_width(width * gtk::pango::SCALE);
        layout.set_wrap(gtk::pango::WrapMode::WordChar);
        let lines = layout.line_count().max(1);
        let line = text_height / lines;
        let last = layout
            .line_readonly(lines - 1)
            .map_or(0, |last| last.extents().1.width() / gtk::pango::SCALE);
        let x = last + self.space();
        if x + tail_width <= width {
            (
                text_height,
                (x, text_height - line, tail_width, line),
                text_height,
            )
        } else {
            (
                text_height,
                (0, text_height, tail_width, line),
                text_height + line,
            )
        }
    }
}

/// `.update-spinner`: a three-quarter ring in `currentColor`, one turn per
/// 0.72 s. Only this small area redraws each frame.
fn spinner(side: i32) -> SnapshotArea {
    let area = SnapshotArea::new(|_, _, _| {});
    area.add_css_class("update-spinner");
    area.set_size_request(side, side);
    let weak = area.downgrade();
    area.set_draw(move |snapshot, w, h| {
        let Some(area) = weak.upgrade() else { return };
        #[allow(deprecated)] // `gtk_widget_get_color` needs GTK 4.10.
        let color = area.style_context().color();
        let cr = snapshot.append_cairo(&graphene::Rect::new(0., 0., w, h));
        let turn = (glib::monotonic_time() as f64 / 720_000.).fract() * TAU;
        cr.set_source_rgba(
            color.red().into(),
            color.green().into(),
            color.blue().into(),
            color.alpha().into(),
        );
        cr.set_line_width(2.);
        let radius = (f64::from(w.min(h)) - 2.) / 2.;
        cr.arc(
            f64::from(w) / 2.,
            f64::from(h) / 2.,
            radius,
            turn,
            turn + 0.75 * TAU,
        );
        let _ = cr.stroke();
    });
    area.add_tick_callback(|area, _| {
        area.queue_draw();
        glib::ControlFlow::Continue
    });
    area
}

/// The New Capture keys the launch notice shows: none on Wayland, where the
/// GlobalShortcuts portal decides the real trigger (as the wgpu host).
pub fn startup_keys(settings: &captures_settings::AppSettings, wayland: bool) -> Vec<String> {
    if wayland {
        return Vec::new();
    }
    let shortcut = Some(settings.new_capture_shortcut.clone())
        .filter(|shortcut| !shortcut.trim().is_empty())
        .unwrap_or_else(captures_settings::default_new_capture_shortcut);
    captures_app::shortcuts::shortcut_display_tokens(
        &shortcut,
        captures_app::shortcuts::ShortcutPlatform::Linux,
    )
}
