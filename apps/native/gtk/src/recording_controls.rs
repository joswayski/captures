//! Live recording controls. New Capture transfers its X11 selection and
//! capture guard here. Sessions and media never live on GTK's
//! thread. The parent capture guard survives pauses, screenshots and hiding.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use captures_app::capture_flow::{self, CaptureFlow};
use captures_app::recording_hud::{self as hud, Control};
use captures_capture::{DisplayDescriptor, XcapBackend};
use captures_recording::{
    AudioOptions, GifOptions, RecordingKind, RecordingOptions, RecordingSessionSnapshot,
    RecordingState, RecordingTarget,
};
use captures_recording_platform::worker::{Command, Event, Worker};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::{Host, emit, widgets::Icon};

const TITLE: &str = "Captures Recording Controls";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pending {
    Prepare,
    Countdown,
    Start,
    None,
    Change,
    Refresh,
    Restart,
    Finish,
    Discard,
    Confirm,
}

pub struct Controls {
    pub window: gtk::ApplicationWindow,
    pub flow: RefCell<CaptureFlow>,
    host: Weak<Host>,
    worker: RefCell<Worker>,
    display: RefCell<Option<DisplayDescriptor>>,
    settings: captures_settings::AppSettings,
    target: Option<RecordingTarget>,
    hidden: Vec<gtk::Window>,
    snapshot: RefCell<Option<RecordingSessionSnapshot>>,
    pending: Cell<Pending>,
    accepted: Cell<bool>,
    closing: Cell<bool>,
    polled: Cell<Instant>,
    poll_pending: Cell<bool>,
    clock: Cell<Instant>,
    elapsed: gtk::Label,
    status: gtk::Label,
    dot: gtk::Box,
    error: gtk::Label,
    error_text: RefCell<hud::ErrorLine>,
    meter: gtk::ProgressBar,
    buttons: Vec<(Control, gtk::Button, Option<Icon>)>,
    tooltip_layer: gtk::Fixed,
    tooltip: gtk::Label,
    tooltip_hover: Cell<Option<Control>>,
    notice: RefCell<Option<gtk::ApplicationWindow>>,
    confirmation: RefCell<Option<gtk::MessageDialog>>,
}

impl Host {
    /// Deliberately not a tray/menu capability yet. This launch flag tests the
    /// real HUD/session boundary without implying capture-menu parity.
    pub fn record_display(self: &Rc<Self>) {
        if self.restore_recording_controls() {
            return;
        }
        if crate::capture::wayland(self) {
            return self.capture_failed("The GTK recording development entry point currently requires X11. Wayland recording is not connected yet.");
        }
        if self.capture.borrow().is_some() {
            return self.capture_failed("A capture is already in progress");
        }
        let flow = match CaptureFlow::begin(0) {
            Ok(flow) => flow,
            Err(error) => return self.capture_failed(&error),
        };
        let controls = Controls::new(
            self,
            flow,
            None,
            None,
            captures_settings::load(&self.settings_file).unwrap_or_default(),
            Vec::new(),
        );
        *self.recorder.borrow_mut() = Some(controls.clone());
        controls.watch();
        glib::spawn_future_local(async move {
            let display = gio::spawn_blocking(|| {
                let id = crate::capture::pointer_display()?;
                XcapBackend
                    .displays()
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .find(|display| display.id == id)
                    .ok_or_else(|| "The selected display is no longer available".to_owned())
            })
            .await
            .unwrap_or_else(|_| Err("The display worker stopped".into()));
            if controls.closing.get() {
                return;
            }
            match display {
                Ok(display) if controls.current() => {
                    *controls.display.borrow_mut() = Some(display);
                    controls.send(Command::VerifyToolchain {
                        generation: controls.generation(),
                    });
                }
                Ok(_) => controls.end(),
                Err(error) => {
                    if let Some(host) = controls.host.upgrade() {
                        host.capture_failed(&error);
                    }
                    controls.end();
                }
            }
        });
    }

    pub fn record_selected(
        self: &Rc<Self>,
        flow: CaptureFlow,
        display: DisplayDescriptor,
        target: RecordingTarget,
        settings: captures_settings::AppSettings,
        hidden: Vec<gtk::Window>,
    ) {
        let controls = Controls::new(self, flow, Some(display), Some(target), settings, hidden);
        *self.recorder.borrow_mut() = Some(controls.clone());
        controls.watch();
        glib::spawn_future_local(async move {
            glib::timeout_future(crate::capture::SETTLE).await;
            controls.send(Command::VerifyToolchain {
                generation: controls.generation(),
            });
        });
    }

    pub fn restore_recording_controls(&self) -> bool {
        if let Some(controls) = self.recorder.borrow().as_ref() {
            controls.restore();
            true
        } else {
            false
        }
    }
}

impl Controls {
    fn watch(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(50), move || {
            let Some(controls) = weak.upgrade().filter(|c| !c.closing.get()) else {
                return glib::ControlFlow::Break;
            };
            controls.drain();
            glib::ControlFlow::Continue
        });
    }

    fn generation(&self) -> u64 {
        self.flow.borrow().generation()
    }
    fn current(&self) -> bool {
        self.flow.borrow().is_current()
    }
    fn send(&self, command: Command) {
        self.worker.borrow().send(command);
    }

    fn new(
        host: &Rc<Host>,
        flow: CaptureFlow,
        display: Option<DisplayDescriptor>,
        target: Option<RecordingTarget>,
        settings: captures_settings::AppSettings,
        hidden: Vec<gtk::Window>,
    ) -> Rc<Self> {
        let window = gtk::ApplicationWindow::builder()
            .application(&host.app)
            .title(TITLE)
            .decorated(false)
            .resizable(false)
            .build();
        window.add_css_class("captures");
        window.add_css_class("recording-controls-window");
        let outer = gtk::Box::new(gtk::Orientation::Vertical, 5);
        let card = gtk::Box::new(gtk::Orientation::Vertical, 2);
        card.add_css_class("recording-hud");
        let copy = captures_app::capture_menu::visibility_note(
            captures_app::capture_menu::MenuMode::Recording,
            false,
            false,
        );
        let privacy = gtk::Label::new(Some(&copy.text()));
        privacy.add_css_class("recording-hud-privacy");
        card.append(&privacy);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.add_css_class("recording-hud-dot");
        dot.set_valign(gtk::Align::Center);
        row.append(&dot);
        let status_column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        status_column.set_size_request(80, -1);
        let elapsed = gtk::Label::new(Some("0:00"));
        elapsed.add_css_class("recording-hud-time");
        elapsed.set_xalign(0.);
        let status = gtk::Label::new(Some("STARTING…"));
        status.add_css_class("recording-hud-status");
        status.set_xalign(0.);
        status_column.append(&elapsed);
        status_column.append(&status);
        row.append(&status_column);
        let meter = gtk::ProgressBar::new();
        meter.add_css_class("recording-hud-meter");
        meter.update_property(&[gtk::accessible::Property::Label("Microphone level")]);
        meter.set_size_request(34, -1);
        meter.set_valign(gtk::Align::Center);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        actions.add_css_class("recording-hud-actions");
        let buttons = Control::ALL
            .into_iter()
            .map(|control| {
                if control == Control::Microphone {
                    actions.append(&meter);
                }
                let button = gtk::Button::new();
                let icon = if control == Control::Stop {
                    button.add_css_class("recording-stop");
                    let square = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                    square.add_css_class("recording-stop-square");
                    square.set_halign(gtk::Align::Center);
                    square.set_valign(gtk::Align::Center);
                    button.set_child(Some(&square));
                    None
                } else {
                    let icon = Icon::new("pause", 16);
                    icon.set_halign(gtk::Align::Center);
                    icon.set_valign(gtk::Align::Center);
                    button.set_child(Some(&icon));
                    Some(icon)
                };
                if control == Control::Delete {
                    button.add_css_class("recording-discard");
                }
                if control == Control::Hide {
                    button.set_margin_start(4);
                }
                actions.append(&button);
                (control, button, icon)
            })
            .collect();
        row.append(&actions);
        card.append(&row);
        outer.append(&card);
        let error = gtk::Label::new(None);
        error.add_css_class("recording-hud-error");
        error.set_ellipsize(gtk::pango::EllipsizeMode::End);
        error.set_max_width_chars(65);
        // Keep the transparent native surface stable when an error arrives.
        error.set_size_request(-1, 16);
        outer.append(&error);
        // Keep tooltips inside the existing transparent HUD, without a popup
        // surface that can steal focus or intercept recording controls.
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&outer));
        let tooltip_layer = gtk::Fixed::new();
        tooltip_layer.set_can_target(false);
        overlay.add_overlay(&tooltip_layer);
        let tooltip = gtk::Label::builder()
            .accessible_role(gtk::AccessibleRole::Tooltip)
            .build();
        tooltip.add_css_class("recording-hud-tooltip");
        tooltip.update_state(&[gtk::accessible::State::Hidden(true)]);
        tooltip_layer.put(&tooltip, 0., 0.);
        window.set_child(Some(&overlay));
        let worker = Worker::new(
            || {
                let (ffmpeg, ffprobe) = crate::recording::media_tools();
                captures_media::MediaToolchain::new(ffmpeg, ffprobe)
            },
            || {},
            capture_flow::is_current,
        );
        let controls = Rc::new(Self {
            window,
            flow: RefCell::new(flow),
            host: Rc::downgrade(host),
            worker: RefCell::new(worker),
            display: RefCell::new(display),
            settings,
            target,
            hidden,
            snapshot: RefCell::default(),
            pending: Cell::new(Pending::Prepare),
            accepted: Cell::new(false),
            closing: Cell::new(false),
            polled: Cell::new(Instant::now()),
            poll_pending: Cell::new(false),
            clock: Cell::new(Instant::now()),
            elapsed,
            status,
            dot,
            error,
            error_text: RefCell::default(),
            meter,
            buttons,
            tooltip_layer,
            tooltip,
            tooltip_hover: Cell::new(None),
            notice: RefCell::default(),
            confirmation: RefCell::default(),
        });
        for (control, button, _) in &controls.buttons {
            let (control, weak) = (*control, Rc::downgrade(&controls));
            button.connect_clicked(move |_| {
                if let Some(controls) = weak.upgrade() {
                    controls.action(control);
                }
            });
            let weak = Rc::downgrade(&controls);
            button.connect_has_focus_notify(move |_| {
                if let Some(controls) = weak.upgrade() {
                    controls.refresh_tooltip();
                }
            });
        }
        // Hit-test at the window so disabled controls also show shipping's
        // immediate tooltip; their accessible names retain the disabled reason.
        let motion = gtk::EventControllerMotion::new();
        motion.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(&controls);
        motion.connect_enter(move |_, x, y| {
            if let Some(controls) = weak.upgrade() {
                controls.hover_tooltip(x, y);
            }
        });
        let weak = Rc::downgrade(&controls);
        motion.connect_motion(move |_, x, y| {
            if let Some(controls) = weak.upgrade() {
                controls.hover_tooltip(x, y);
            }
        });
        let weak = Rc::downgrade(&controls);
        motion.connect_leave(move |_| {
            if let Some(controls) = weak.upgrade() {
                controls.tooltip_hover.set(None);
                controls.refresh_tooltip();
            }
        });
        controls.window.add_controller(motion);
        let weak = Rc::downgrade(&controls);
        controls.window.connect_unmap(move |_| {
            if let Some(controls) = weak.upgrade() {
                controls.tooltip_hover.set(None);
                controls.refresh_tooltip();
            }
        });
        let weak = Rc::downgrade(&controls);
        controls.window.connect_close_request(move |_| {
            if let Some(controls) = weak.upgrade() {
                controls.action(Control::Stop);
            }
            glib::Propagation::Stop
        });
        // Drag only the status/blank chrome, never a button or meter.
        let drag = gtk::GestureClick::new();
        drag.set_button(gdk::BUTTON_PRIMARY);
        status_column.add_controller(drag.clone());
        let window = controls.window.downgrade();
        drag.connect_pressed(move |gesture, _, x, y| {
            if let Some(window) = window.upgrade()
                && let Some(surface) = window.surface().and_downcast::<gdk::Toplevel>()
                && let Some(device) = gesture.current_event_device()
            {
                let origin = status_column
                    .compute_point(&window, &gtk::graphene::Point::new(x as f32, y as f32));
                if let Some(point) = origin {
                    surface.begin_move(
                        &device,
                        1,
                        f64::from(point.x()),
                        f64::from(point.y()),
                        gesture.current_event_time(),
                    );
                }
            }
        });
        controls.place();
        controls.refresh();
        controls
    }

    fn input(&self) -> hud::Input {
        let snapshot = self.snapshot.borrow();
        let child = self
            .host
            .upgrade()
            .is_some_and(|host| host.capture.borrow().is_some());
        hud::Input {
            state: if self.pending.get() == Pending::Finish {
                RecordingState::Finalizing
            } else {
                snapshot
                    .as_ref()
                    .map_or(RecordingState::Countdown, |s| s.state)
            },
            busy: self.pending.get() != Pending::None || child,
            has_microphone: snapshot
                .as_ref()
                .is_some_and(|s| s.options.audio.microphone_device_id.is_some()),
            microphone_muted: snapshot
                .as_ref()
                .is_some_and(|s| s.options.audio.microphone_muted),
            hide_available: self
                .host
                .upgrade()
                .is_some_and(|host| host.tray.borrow().is_some()),
        }
    }

    fn refresh(&self) {
        let input = self.input();
        let view = hud::present(&input);
        self.status.set_label(&view.status_label.to_uppercase());
        let mut classes = vec![
            "recording-hud-dot",
            match view.dot {
                hud::Dot::Signal => "signal",
                hud::Dot::Accent => "accent",
                hud::Dot::Info => "info",
                hud::Dot::Subtle => "subtle",
            },
        ];
        if view.pulsing && self.dot.settings().is_gtk_enable_animations() {
            classes.push("pulsing");
        }
        self.dot.set_css_classes(&classes);
        let elapsed = self.snapshot.borrow().as_ref().map_or(0, |s| s.elapsed_ms)
            + if view.timer_running && self.pending.get() != Pending::Finish {
                self.clock.get().elapsed().as_millis() as u64
            } else {
                0
            };
        let seconds = elapsed / 1000;
        self.elapsed.set_label(&if seconds >= 3600 {
            format!(
                "{}:{:02}:{:02}",
                seconds / 3600,
                seconds / 60 % 60,
                seconds % 60
            )
        } else {
            format!("{}:{:02}", seconds / 60, seconds % 60)
        });
        self.meter.set_visible(view.show_meter);
        for (control, button, icon) in &self.buttons {
            let state = view.control(*control);
            button.set_sensitive(state.enabled);
            button.update_property(&[gtk::accessible::Property::Label(state.label)]);
            if state.selected {
                button.add_css_class("active");
            } else {
                button.remove_css_class("active");
            }
            if let (Some(icon), Some(name)) = (icon, state.icon) {
                icon.set_name(name);
            }
        }
        let snapshot = self.snapshot.borrow();
        let error = self
            .error_text
            .borrow()
            .line(snapshot.as_ref().and_then(|s| s.error.as_deref()));
        self.error
            .set_label(&error.as_deref().map(hud::error_message).unwrap_or_default());
        self.error
            .set_opacity(if error.is_some() { 1. } else { 0. });
        self.error
            .update_state(&[gtk::accessible::State::Hidden(error.is_none())]);
        self.refresh_tooltip();
    }

    fn hover_tooltip(&self, x: f64, y: f64) {
        let point = gtk::graphene::Point::new(x as f32, y as f32);
        let control = self.buttons.iter().find_map(|(control, button, _)| {
            button
                .compute_bounds(&self.window)
                .filter(|bounds| bounds.contains_point(&point))
                .map(|_| *control)
        });
        self.tooltip_hover.set(control);
        self.refresh_tooltip();
    }

    fn refresh_tooltip(&self) {
        use captures_app::tray_notice::LogicalRect;

        if self.tooltip.settings().is_gtk_enable_animations() {
            self.tooltip.add_css_class("animated");
        } else {
            self.tooltip.remove_css_class("animated");
        }
        // Read actual focus, including after sensitivity changes during a
        // recording command, rather than retaining a separate focus owner.
        let owner = self
            .tooltip_hover
            .get()
            .or_else(|| {
                self.buttons
                    .iter()
                    .find_map(|(control, button, _)| button.has_focus().then_some(*control))
            })
            .filter(|_| self.window.is_mapped());
        let Some(control) = owner else {
            self.tooltip.remove_css_class("showing");
            self.tooltip
                .update_state(&[gtk::accessible::State::Hidden(true)]);
            return;
        };
        let (_, button, _) = self.buttons.iter().find(|(c, _, _)| *c == control).unwrap();
        let Some(anchor) = button.compute_bounds(&self.tooltip_layer) else {
            return;
        };
        let view = hud::present(&self.input());
        self.tooltip.set_label(view.control(control).tooltip);
        let (_, width, _, _) = self.tooltip.measure(gtk::Orientation::Horizontal, -1);
        let (_, height, _, _) = self.tooltip.measure(gtk::Orientation::Vertical, width);
        let frame = hud::tooltip_frame(
            LogicalRect::new(
                anchor.x().into(),
                anchor.y().into(),
                anchor.width().into(),
                anchor.height().into(),
            ),
            f64::from(width) - hud::TOOLTIP_PADDING_X * 2. - 2.,
            f64::from(height) - hud::TOOLTIP_PADDING_Y * 2. - 2.,
            control.tooltip_right_aligned(),
            1.,
            LogicalRect::new(
                0.,
                0.,
                self.window.width().into(),
                self.window.height().into(),
            ),
        );
        self.tooltip_layer.move_(&self.tooltip, frame.x, frame.y);
        self.tooltip
            .update_state(&[gtk::accessible::State::Hidden(false)]);
        self.tooltip.add_css_class("showing");
    }

    fn drain(self: &Rc<Self>) {
        loop {
            let event = self.worker.borrow().try_recv();
            let Some(event) = event else { break };
            self.event(event);
            if self.closing.get() {
                return;
            }
        }
        if !self.current()
            && !matches!(
                self.pending.get(),
                Pending::Finish | Pending::Discard | Pending::Countdown
            )
        {
            // Session lock/loss saves an accepted take. Escape before acceptance
            // cancels only the pending take, never a previously accepted one.
            if self.snapshot.borrow().is_some() {
                self.stop_cancelled();
            }
        }
        if self.pending.get() == Pending::None
            && self.accepted.get()
            && !self.poll_pending.get()
            && self.polled.get().elapsed() >= Duration::from_millis(500)
        {
            self.poll_pending.set(true);
            self.polled.set(Instant::now());
            self.send(Command::Snapshot {
                generation: self.generation(),
            });
        }
        self.refresh();
    }

    fn stop_cancelled(&self) {
        let generation = self.generation();
        if self.accepted.get() {
            self.finish();
        } else {
            self.pending.set(Pending::Discard);
            self.send(Command::Discard { generation });
        }
    }

    fn finish(&self) {
        if let Some(snapshot) = self.snapshot.borrow_mut().as_mut()
            && snapshot.state == RecordingState::Recording
        {
            snapshot.elapsed_ms += self.clock.get().elapsed().as_millis() as u64;
        }
        self.clock.set(Instant::now());
        if let Some(host) = self.host.upgrade() {
            self.pending.set(Pending::Finish);
            self.send(Command::Finish {
                generation: self.generation(),
                history_root: host.history_root.clone(),
            });
        }
    }

    fn snapshot(&self, snapshot: RecordingSessionSnapshot) {
        self.clock.set(Instant::now());
        self.error_text
            .borrow_mut()
            .apply(hud::ErrorEvent::Warning {
                warning: snapshot.warning.clone(),
            });
        emit(
            "recording-state",
            serde_json::json!({"state": snapshot.state, "elapsedMs": snapshot.elapsed_ms}),
        );
        *self.snapshot.borrow_mut() = Some(snapshot);
    }

    fn event(self: &Rc<Self>, event: Event) {
        // A session-loss decision can be queued behind a start/pause/snapshot.
        // That earlier reply must not reopen the HUD or submit a second finish.
        if matches!(self.pending.get(), Pending::Finish | Pending::Discard)
            && !matches!(event, Event::Finished { .. } | Event::Discarded { .. })
        {
            return;
        }
        match event {
            Event::ToolchainVerified { result, .. } => match result {
                Ok(()) if self.current() => {
                    let display = self
                        .display
                        .borrow()
                        .clone()
                        .expect("display before verification");
                    let settings = &self.settings.recording;
                    let options = RecordingOptions {
                        kind: RecordingKind::Video,
                        target: self
                            .target
                            .clone()
                            .unwrap_or_else(|| RecordingTarget::Display {
                                display_id: display.id.clone(),
                            }),
                        frames_per_second: settings.video_fps,
                        max_resolution: settings.video_max_resolution,
                        countdown_seconds: settings.countdown_seconds,
                        show_cursor: settings.show_cursor,
                        highlight_clicks: settings.highlight_clicks,
                        show_keystrokes: settings.show_keystrokes,
                        audio: AudioOptions {
                            capture_system_audio: settings.capture_system_audio,
                            microphone_device_id: settings.microphone_device_id.clone(),
                            mono_output: settings.mono_audio,
                            ..AudioOptions::default()
                        },
                        gif: GifOptions::default(),
                    };
                    if let Some(host) = self.host.upgrade() {
                        self.send(Command::Prepare {
                            generation: self.generation(),
                            options,
                            recovery_root: host.history_root.with_file_name("recording-recovery"),
                            display: Some(display),
                        });
                    }
                }
                result => {
                    if let Err(error) = result
                        && let Some(host) = self.host.upgrade()
                    {
                        host.capture_failed(&error);
                    }
                    self.end();
                }
            },
            Event::Prepared { result, .. } => match result {
                Ok(snapshot) => {
                    self.snapshot(snapshot);
                    self.countdown(false);
                }
                Err(error) => {
                    if let Some(host) = self.host.upgrade() {
                        host.capture_failed(&error);
                    }
                    self.end();
                }
            },
            Event::Restarted { result, .. } => match result {
                Ok(snapshot) => {
                    self.error_text
                        .borrow_mut()
                        .apply(hud::ErrorEvent::SessionChanged);
                    self.snapshot(snapshot);
                    self.accepted.set(false);
                    self.countdown(true);
                }
                Err(error) => self.failed(error),
            },
            Event::Started { result, .. } => match result {
                Ok(snapshot) => {
                    self.snapshot(snapshot);
                    let accepted = self.flow.borrow_mut().disarm_escape().is_ok();
                    if accepted {
                        self.accepted.set(true);
                        self.pending.set(Pending::None);
                        self.restore();
                    } else {
                        self.stop_cancelled();
                    }
                }
                Err(failure) => {
                    if let Some(snapshot) = failure.snapshot {
                        self.snapshot(*snapshot);
                    }
                    if !self.current() {
                        self.stop_cancelled();
                    } else {
                        self.failed(failure.error);
                    }
                }
            },
            Event::Paused { result, .. } => match result {
                Ok(snapshot) => {
                    self.snapshot(snapshot);
                    self.pending.set(Pending::None);
                }
                Err(error) => self.failed(error),
            },
            Event::MicrophoneMuted { result, .. } => match result {
                Ok(snapshot) => {
                    self.snapshot(snapshot);
                    self.pending.set(Pending::None);
                }
                Err(failure) => {
                    if let Some(snapshot) = failure.snapshot {
                        self.snapshot(*snapshot);
                    }
                    self.failed(failure.error);
                }
            },
            Event::Snapshot {
                result,
                microphone_peak,
                ..
            } => {
                self.poll_pending.set(false);
                self.meter
                    .set_fraction(f64::from(microphone_peak.clamp(0., 1.)));
                match result {
                    Ok(snapshot) => {
                        let failed = snapshot.state == RecordingState::Failed
                            && self
                                .snapshot
                                .borrow()
                                .as_ref()
                                .is_none_or(|s| s.state != RecordingState::Failed);
                        self.snapshot(snapshot);
                        let refreshed = self.pending.get() == Pending::Refresh;
                        if refreshed {
                            self.pending.set(Pending::None);
                        }
                        if failed || refreshed {
                            self.restore();
                        }
                    }
                    Err(error) => {
                        // No session snapshot is available. Keep recovery and
                        // expose Retry/Delete rather than repeatedly querying.
                        self.pending.set(Pending::None);
                        if let Some(snapshot) = self.snapshot.borrow_mut().as_mut() {
                            snapshot.state = RecordingState::Failed;
                        }
                        self.error_text
                            .borrow_mut()
                            .apply(hud::ErrorEvent::ActionFailed { message: error });
                        self.restore();
                    }
                }
            }
            Event::Finished { result, .. } => match result {
                Ok(recording) => {
                    if let Some(host) = self.host.upgrade() {
                        emit(
                            "recording-saved",
                            serde_json::json!({"id": recording.entry.id}),
                        );
                        if let Some(history) = host.history.borrow().as_ref() {
                            history.reload();
                        }
                        if self.settings.show_mini_previews {
                            host.show_preview(recording.entry.clone());
                        }
                        if self.settings.recording.open_editor_after_recording {
                            host.show_recording(&recording.entry.id);
                        }
                        if let Some(warning) = recording.warning {
                            host.show_error(&warning);
                        }
                    }
                    self.end();
                }
                Err(error) => self.terminal_error(error),
            },
            Event::Discarded { result, .. } => match result {
                Ok(_) => {
                    emit("recording-discarded", serde_json::json!({}));
                    self.end();
                }
                Err(error) => self.terminal_error(error),
            },
            Event::SelectRegion { reply, .. } => {
                let _ = reply.send(None);
            }
            Event::Microphones { .. } | Event::Retired => {}
        }
        if !self.closing.get() {
            self.refresh();
        }
    }

    fn countdown(self: &Rc<Self>, restart: bool) {
        let seconds = self.settings.recording.countdown_seconds;
        let result = if restart {
            self.flow.borrow_mut().restart_countdown(seconds)
        } else {
            self.flow.borrow_mut().start_countdown(seconds)
        };
        if let Err(error) = result {
            if self.current() {
                self.failed(error);
            }
            return;
        }
        self.pending.set(Pending::Countdown);
        self.window.set_visible(false);
        let controls = self.clone();
        glib::spawn_future_local(async move {
            if let Some(host) = controls.host.upgrade() {
                let display = controls.display.borrow().clone();
                crate::countdown::wait_recording(
                    &host,
                    controls.generation(),
                    seconds,
                    display.as_ref(),
                )
                .await;
                glib::timeout_future(crate::capture::SETTLE).await;
                if !controls.closing.get() && controls.pending.get() == Pending::Countdown {
                    if controls.current() {
                        controls.pending.set(Pending::Start);
                        controls.send(Command::Start {
                            generation: controls.generation(),
                            exclude_captures_app: false,
                        });
                    } else {
                        controls.stop_cancelled();
                    }
                }
            }
        });
    }

    fn terminal_error(&self, error: String) {
        if self.current() {
            self.failed(error);
        } else {
            if let Some(host) = self.host.upgrade() {
                host.show_error(&format!(
                    "{error}\nThe recording recovery files have been retained."
                ));
            }
            emit(
                "recording-recovery-retained",
                serde_json::json!({"error": error}),
            );
            self.end();
        }
    }

    fn failed(&self, error: String) {
        // A failed resume/microphone reopen can leave a valid paused take.
        // Ask the session for its actual state before enabling any actions;
        // never turn a recoverable action error into a failed recording.
        self.pending.set(Pending::Refresh);
        self.poll_pending.set(true);
        self.send(Command::Snapshot {
            generation: self.generation(),
        });
        self.error_text
            .borrow_mut()
            .apply(hud::ErrorEvent::ActionFailed {
                message: error.clone(),
            });
        // A failed start still has Retry/Delete, and Escape must not quietly
        // throw away its recovery bundle while the error is being read.
        let _ = self.flow.borrow_mut().disarm_escape();
        emit("recording-failed", serde_json::json!({"error": error}));
        self.restore();
    }

    fn action(self: &Rc<Self>, control: Control) {
        let input = self.input();
        let view = hud::present(&input);
        if !view.control(control).enabled {
            return;
        }
        self.error_text
            .borrow_mut()
            .apply(hud::ErrorEvent::ActionStarted);
        let generation = self.generation();
        match control {
            Control::Stop => self.finish(),
            Control::PauseResume => {
                self.pending.set(Pending::Change);
                self.send(if input.state == RecordingState::Paused {
                    Command::Resume {
                        generation,
                        exclude_captures_app: false,
                    }
                } else {
                    Command::Pause { generation }
                });
            }
            Control::Microphone => {
                self.pending.set(Pending::Change);
                self.send(Command::SetMicrophoneMuted {
                    generation,
                    muted: !input.microphone_muted,
                    exclude_captures_app: false,
                });
            }
            Control::Restart if !view.restart_confirms => {
                self.pending.set(Pending::Restart);
                self.send(Command::Restart { generation });
            }
            Control::Restart | Control::Delete => self.confirm(control),
            Control::Screenshot => {
                if let Some(host) = self.host.upgrade() {
                    host.capture_region();
                }
            }
            Control::Hide => {
                self.window.set_visible(false);
                crate::live::recording_hidden(true);
                self.hidden_notice();
            }
        }
        self.refresh();
    }

    #[allow(deprecated)] // The same native confirmation as shipping; GTK 4.8.
    fn confirm(self: &Rc<Self>, control: Control) {
        let restart = control == Control::Restart;
        let dialog = gtk::MessageDialog::builder()
            .application(&self.window.application().expect("recording app"))
            .transient_for(&self.window)
            .modal(true)
            .message_type(gtk::MessageType::Question)
            .text(if restart {
                "Restart recording?"
            } else {
                "Delete recording?"
            })
            .secondary_text(if restart {
                "The current recording will be deleted and a new countdown will begin."
            } else {
                "This recording will be deleted permanently."
            })
            .build();
        dialog.add_button("Cancel", gtk::ResponseType::Cancel);
        dialog.add_button(
            if restart { "Restart" } else { "Delete" },
            gtk::ResponseType::Accept,
        );
        dialog.set_default_response(gtk::ResponseType::Cancel);
        self.pending.set(Pending::Confirm);
        let weak = Rc::downgrade(self);
        dialog.connect_response(move |dialog, response| {
            dialog.destroy();
            if let Some(controls) = weak.upgrade() {
                controls.confirmation.borrow_mut().take();
                if controls.closing.get() || controls.pending.get() != Pending::Confirm {
                    return;
                }
                if response == gtk::ResponseType::Accept {
                    controls.pending.set(if restart {
                        Pending::Restart
                    } else {
                        Pending::Discard
                    });
                    controls.send(if restart {
                        Command::Restart {
                            generation: controls.generation(),
                        }
                    } else {
                        Command::Discard {
                            generation: controls.generation(),
                        }
                    });
                } else {
                    controls.pending.set(Pending::None);
                }
                controls.refresh();
            }
        });
        *self.confirmation.borrow_mut() = Some(dialog.clone());
        dialog.present();
    }

    pub fn restore(&self) {
        self.notice
            .borrow_mut()
            .take()
            .inspect(|notice| notice.close());
        crate::live::recording_hidden(false);
        if self.accepted.get() || self.input().state == RecordingState::Failed {
            self.window.present();
        }
    }

    fn hidden_notice(&self) {
        let Some(host) = self.host.upgrade() else {
            return;
        };
        let window = crate::notices::recording_hidden(
            &host.app,
            &self.settings.new_capture_shortcut,
            &self.window,
        );
        window.set_visible(true);
        *self.notice.borrow_mut() = Some(window);
    }

    fn place(&self) {
        #[cfg(feature = "x11")]
        if let Some(placer) =
            crate::previews::x11::Placer::new(&WidgetExt::display(&self.window)).map(Rc::new)
        {
            self.window.connect_realize(move |window| {
                if let Some(surface) = window.surface().and_downcast::<gdk4_x11::X11Surface>() {
                    surface.set_skip_taskbar_hint(true);
                    surface.set_skip_pager_hint(true);
                    let (pin, weak) = (placer.clone(), surface.downgrade());
                    placer.watch_managed(&surface, move || {
                        if let Some(surface) = weak.upgrade() {
                            pin.pin(&surface);
                            if let Some((_, area)) = pin.areas(&surface) {
                                pin.move_to(
                                    &surface,
                                    area.x + (area.width - f64::from(surface.width())) / 2.,
                                    area.y + area.height - f64::from(surface.height()) - 24.,
                                );
                            }
                        }
                    });
                }
            });
        }
    }

    fn end(&self) {
        self.closing.set(true);
        crate::live::recording_hidden(false);
        self.flow.borrow().cancel();
        self.send(Command::Retire);
        self.window.destroy();
        self.notice
            .borrow_mut()
            .take()
            .inspect(|notice| notice.destroy());
        let dialog = self.confirmation.borrow_mut().take();
        if let Some(dialog) = dialog {
            dialog.destroy();
        }
        if let Some(host) = self.host.upgrade() {
            host.recorder.borrow_mut().take();
        }
        for window in &self.hidden {
            window.set_visible(true);
        }
    }

    /// SIGTERM/normal Quit saves accepted media before joining the worker.
    /// A failed take is retained for recovery, never interpreted as Discard.
    pub fn shutdown(&self) {
        self.closing.set(true);
        self.flow.borrow().cancel();
        let generation = self.generation();
        let state = self.input().state;
        if !matches!(self.pending.get(), Pending::Finish | Pending::Discard) {
            if self.accepted.get() && state != RecordingState::Failed {
                if let Some(host) = self.host.upgrade() {
                    self.send(Command::Finish {
                        generation,
                        history_root: host.history_root.clone(),
                    });
                }
            } else if !self.accepted.get() && state != RecordingState::Failed {
                self.send(Command::Discard { generation });
            }
        }
        self.worker.borrow_mut().shutdown();
        while let Some(event) = self.worker.borrow().try_recv() {
            if let Event::Finished { result, .. } = event {
                match result {
                    Ok(recording) => emit(
                        "recording-saved",
                        serde_json::json!({"id": recording.entry.id, "onQuit": true}),
                    ),
                    Err(error) => emit(
                        "recording-recovery-retained",
                        serde_json::json!({"error": error}),
                    ),
                }
            }
        }
    }
}
