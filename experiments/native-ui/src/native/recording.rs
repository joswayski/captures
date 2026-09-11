//! Native GTK recording HUD and media editor.
//!
//! Pausing ends the current native xcap segment and resuming starts another.
//! This is intentional: `XcapRecordingSegment` owns the capture and audio
//! workers, and `MediaToolchain::assemble_recording_segments` is the canonical
//! way to preserve and align those segment audio streams.
use captures_capture::DisplayDescriptor;
use captures_media::{
    AudioEdit, CancelToken, CropRect, EditSpec, ExportFormat, ExportSpec, MediaToolchain,
    QualityPreset, RecordingAudioLayout, RecordingSegmentInput,
};
use captures_recording::{
    DraftStore, RecordingDraftManifest, RecordingKind, RecordingOptions, RecordingSegmentInfo,
    RecordingSegmentManifest, RecordingState, RecordingTarget,
};
use captures_recording_xcap::XcapRecordingSegment;
use gtk::{glib, prelude::*};
#[cfg(unix)]
use std::os::unix::fs::DirBuilderExt;
use std::{
    cell::{Cell, RefCell},
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::ui;

#[path = "recording/editor.rs"]
mod editor;

pub type ScreenshotFinished = Rc<dyn Fn()>;
pub type ScreenshotAction = Rc<dyn Fn(ScreenshotFinished)>;

struct Session {
    options: RecordingOptions,
    output_format: ExportFormat,
    display: DisplayDescriptor,
    output_directory: PathBuf,
    work_directory: PathBuf,
    draft_store: DraftStore,
    manifest: RecordingDraftManifest,
    active: Option<XcapRecordingSegment>,
    segments: Vec<RecordingSegmentInfo>,
    started: Instant,
    elapsed_before_segment: Duration,
    busy: bool,
    closing: bool,
    safety_query_in_flight: bool,
    screenshot_in_flight: bool,
    region_border: Option<RegionBorder>,
    include_controls: bool,
    on_finished: Option<Rc<dyn Fn()>>,
}

#[derive(Clone)]
pub struct RecordingActions {
    /// Starts the app-owned screenshot flow after recording is safely paused.
    /// The callback must invoke its supplied completion callback on save or cancel.
    pub screenshot: Option<ScreenshotAction>,
    /// Runs exactly once when recording reaches a terminal path.
    pub on_finished: Option<Rc<dyn Fn()>>,
    /// Preferred saved-video container. Linux currently ships MP4 and GIF export.
    pub preferred_video_format: ExportFormat,
    pub include_controls: bool,
}

impl Default for RecordingActions {
    fn default() -> Self {
        Self {
            screenshot: None,
            on_finished: None,
            preferred_video_format: ExportFormat::Mp4,
            include_controls: false,
        }
    }
}

/// Start recording with optional HUD actions supplied by the owning app shell.
pub fn start_with_actions(
    options: RecordingOptions,
    display: DisplayDescriptor,
    directory: PathBuf,
    on_saved: Rc<dyn Fn(PathBuf)>,
    actions: RecordingActions,
) {
    let output_format = match recording_output_format(&options, actions.preferred_video_format) {
        Ok(format) => format,
        Err(error) => {
            let parent = gtk::Window::new(gtk::WindowType::Toplevel);
            ui::error(&parent, &error);
            parent.close();
            if let Some(on_finished) = actions.on_finished {
                on_finished();
            }
            return;
        }
    };
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("Captures recording controls");
    window.set_keep_above(true);
    window.set_decorated(false);
    window.set_resizable(false);
    window.set_type_hint(gtk::gdk::WindowTypeHint::Utility);
    window.set_skip_taskbar_hint(true);
    window.set_default_size(510, 82);
    window.set_app_paintable(true);
    window.style_context().add_class("recording-hud-window");
    if let Some(visual) =
        gtk::prelude::WidgetExt::screen(&window).and_then(|screen| screen.rgba_visual())
    {
        window.set_visual(Some(&visual));
    }
    install_hud_transparency();
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.set_margin_top(6);
    root.set_margin_bottom(6);
    root.set_margin_start(6);
    root.set_margin_end(6);
    root.style_context().add_class("glass");
    let content = gtk::Box::new(gtk::Orientation::Vertical, 2);
    content.set_border_width(6);
    let privacy = ui::label(
        "These controls will show in recordings · Use Hide controls to keep them out",
        "muted",
    );
    privacy.set_xalign(0.5);
    privacy.set_tooltip_text(Some(
        "Linux does not provide reliable recording-window exclusion for this backend.",
    ));
    if cfg!(target_os = "windows") {
        privacy.set_text(if actions.include_controls {
            "These controls will show in recordings"
        } else {
            "Controls are excluded from captures"
        });
        privacy.set_tooltip_text(Some("Change Include recording controls in captures in Preferences before starting a recording."));
    }
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.set_halign(gtk::Align::Center);
    row.style_context().add_class("recording-hud-main");
    let status = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    status.set_size_request(96, 38);
    status.style_context().add_class("recording-hud-status");
    let dot = ui::label("●", "recording-dot");
    let elapsed = ui::label("Starting…", "title");
    elapsed.set_width_chars(6);
    let state_label = ui::label("RECORDING", "muted");
    let status_copy = gtk::Box::new(gtk::Orientation::Vertical, 0);
    status_copy.pack_start(&elapsed, true, true, 0);
    status_copy.pack_start(&state_label, true, true, 0);
    status.pack_start(&dot, false, false, 0);
    status.pack_start(&status_copy, false, false, 0);
    let stop = icon_button("stop", "Stop and save recording");
    stop.style_context().add_class("recording-stop");
    let pause = icon_button("pause", "Pause recording");
    let restart = icon_button("restart", "Restart recording");
    let screenshot = icon_button("screenshot", "Take a region screenshot");
    screenshot.set_sensitive(actions.screenshot.is_some());
    let has_microphone = options.audio.microphone_device_id.is_some();
    let microphone = icon_button("microphone", "Mute microphone");
    microphone.set_sensitive(has_microphone);
    let discard = icon_button("trash", "Delete recording");
    let hide = icon_button("hide", "Hide recording controls");
    let actions_row = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    actions_row
        .style_context()
        .add_class("recording-hud-actions");
    row.pack_start(&status, false, false, 0);
    for child in [&stop, &pause, &restart, &screenshot] {
        actions_row.pack_start(child, false, false, 0);
    }
    if has_microphone {
        let microphone_level = gtk::DrawingArea::new();
        microphone_level.set_size_request(34, 4);
        microphone_level.set_tooltip_text(Some(
            "Live microphone level is unavailable in this experiment",
        ));
        microphone_level
            .style_context()
            .add_class("recording-microphone-level");
        if let Some(accessible) = microphone_level.accessible() {
            accessible.set_name("Live microphone level unavailable");
        }
        actions_row.pack_start(&microphone_level, false, false, 8);
    }
    for child in [&microphone, &discard, &hide] {
        actions_row.pack_start(child, false, false, 0);
    }
    row.pack_start(&actions_row, false, false, 0);
    content.pack_start(&privacy, false, false, 0);
    content.pack_start(&row, false, false, 0);
    root.pack_start(&content, true, true, 0);
    window.add(&root);
    #[cfg(target_os = "windows")]
    if let Err(error) = crate::windows::exclude_from_capture(&window, !actions.include_controls) {
        ui::error(&window, &error);
        window.close();
        if let Some(on_finished) = actions.on_finished {
            on_finished();
        }
        return;
    }
    window.show_all();

    let draft_options = recording_manifest_options(&options, output_format);
    let (draft_store, manifest, work_directory) =
        match create_recording_draft(&directory, &draft_options) {
            Ok(value) => value,
            Err(error) => {
                ui::error(&window, &error);
                window.close();
                if let Some(on_finished) = actions.on_finished {
                    on_finished();
                }
                return;
            }
        };
    let region_border =
        RegionBorder::for_target(&options.target, &display, actions.include_controls);
    let session = Rc::new(RefCell::new(Session {
        options,
        output_format,
        display,
        output_directory: directory,
        work_directory,
        draft_store,
        manifest,
        active: None,
        segments: Vec::new(),
        started: Instant::now(),
        elapsed_before_segment: Duration::ZERO,
        busy: false,
        closing: false,
        safety_query_in_flight: false,
        screenshot_in_flight: false,
        region_border,
        include_controls: actions.include_controls,
        on_finished: actions.on_finished,
    }));
    set_controls(&pause, &stop, &discard, false);
    begin_segment(&window, &elapsed, &pause, &stop, &discard, &session);

    {
        let session = session.clone();
        let elapsed = elapsed.clone();
        let state_label = state_label.clone();
        glib::timeout_add_local(Duration::from_millis(200), move || {
            let state = session.borrow();
            if state.closing {
                return glib::ControlFlow::Break;
            }
            if !state.busy {
                let duration = state.elapsed_before_segment
                    + state
                        .active
                        .as_ref()
                        .map_or(Duration::ZERO, |_| state.started.elapsed());
                elapsed.set_text(&format_duration(duration));
                if let Some(warning) = state
                    .active
                    .as_ref()
                    .and_then(XcapRecordingSegment::warning)
                {
                    elapsed.set_tooltip_text(Some(&warning));
                }
                state_label.set_text(if state.active.is_some() {
                    "RECORDING"
                } else {
                    "PAUSED"
                });
            }
            glib::ControlFlow::Continue
        });
    }
    if let Some(take_screenshot) = actions.screenshot {
        let (window, elapsed, pause, stop, discard, screenshot, session) = (
            window.clone(),
            elapsed.clone(),
            pause.clone(),
            stop.clone(),
            discard.clone(),
            screenshot.clone(),
            session.clone(),
        );
        screenshot.clone().connect_clicked(move |_| {
            begin_screenshot_action(
                &window,
                &elapsed,
                &pause,
                &stop,
                &discard,
                &screenshot,
                &session,
                take_screenshot.clone(),
            );
        });
    }
    {
        let (window, elapsed, pause, stop, discard, session, microphone) = (
            window.clone(),
            elapsed.clone(),
            pause.clone(),
            stop.clone(),
            discard.clone(),
            session.clone(),
            microphone.clone(),
        );
        microphone.clone().connect_clicked(move |_| {
            toggle_microphone(
                &window,
                &elapsed,
                &pause,
                &stop,
                &discard,
                &microphone,
                &session,
            );
        });
    }
    {
        let (window, elapsed, pause, stop, discard, session) = (
            window.clone(),
            elapsed.clone(),
            pause.clone(),
            stop.clone(),
            discard.clone(),
            session.clone(),
        );
        restart.connect_clicked(move |_| {
            let dialog = gtk::MessageDialog::new(
                Some(&window),
                gtk::DialogFlags::MODAL,
                gtk::MessageType::Warning,
                gtk::ButtonsType::OkCancel,
                "Restart recording? The current segments will be deleted.",
            );
            let confirmed = dialog.run() == gtk::ResponseType::Ok;
            dialog.close();
            if confirmed {
                restart_session(&window, &elapsed, &pause, &stop, &discard, &session);
            }
        });
    }
    {
        let (window, elapsed, pause, stop, discard, session) = (
            window.clone(),
            elapsed.clone(),
            pause.clone(),
            stop.clone(),
            discard.clone(),
            session.clone(),
        );
        pause.clone().connect_clicked(move |_| {
            if session.borrow().active.is_some() {
                pause_segment(&window, &elapsed, &pause, &stop, &discard, &session);
            } else {
                begin_segment(&window, &elapsed, &pause, &stop, &discard, &session);
            }
        });
    }
    {
        let session = session.clone();
        let window = window.clone();
        let on_saved = on_saved.clone();
        let pause = pause.clone();
        let stop_button = stop.clone();
        let discard = discard.clone();
        stop.connect_clicked(move |_| {
            finish(
                &window,
                &pause,
                &stop_button,
                &discard,
                &session,
                on_saved.clone(),
            )
        });
    }
    {
        let window = window.clone();
        hide.connect_clicked(move |button| {
            window.hide();
            let notice = gtk::Window::new(gtk::WindowType::Toplevel);
            notice.set_title("Captures recording");
            notice.set_keep_above(true);
            notice.set_decorated(false);
            let show = ui::button("Recording • Show controls");
            show.style_context().add_class("primary");
            notice.add(&show);
            #[cfg(target_os = "windows")]
            if let Err(error) = crate::windows::exclude_from_capture(&notice, true) {
                notice.close();
                window.show_all();
                ui::error(&window, &error);
                return;
            }
            notice.show_all();
            let window = window.clone();
            show.connect_clicked(move |_| {
                notice.close();
                window.show_all();
            });
            button.set_sensitive(true);
        });
    }
    {
        let session = session.clone();
        let window = window.clone();
        discard.connect_clicked(move |_| {
            let dialog = gtk::MessageDialog::new(
                Some(&window),
                gtk::DialogFlags::MODAL,
                gtk::MessageType::Warning,
                gtk::ButtonsType::OkCancel,
                "Delete recording? The recording and its recovery draft will be removed.",
            );
            let confirmed = dialog.run() == gtk::ResponseType::Ok;
            dialog.close();
            if confirmed {
                discard_session(&window, &session);
            }
        });
    }
    {
        let session = session.clone();
        let window_for_close = window.clone();
        window.connect_delete_event(move |_, _| {
            if session.borrow().closing {
                return glib::Propagation::Proceed;
            }
            discard_session(&window_for_close, &session);
            glib::Propagation::Stop
        });
    }
    {
        let (window, elapsed, pause, stop, discard, session) = (
            window.clone(),
            elapsed.clone(),
            pause.clone(),
            stop.clone(),
            discard.clone(),
            session.clone(),
        );
        glib::timeout_add_local(Duration::from_secs(1), move || {
            {
                let mut state = session.borrow_mut();
                if state.closing {
                    return glib::ControlFlow::Break;
                }
                if state.active.is_none() || state.busy || state.safety_query_in_flight {
                    return glib::ControlFlow::Continue;
                }
                state.safety_query_in_flight = true;
            }
            let (window, elapsed, pause, stop, discard, session) = (
                window.clone(),
                elapsed.clone(),
                pause.clone(),
                stop.clone(),
                discard.clone(),
                session.clone(),
            );
            ui::job(
                || Ok(captures_session::capture_session_available()),
                move |result| {
                    session.borrow_mut().safety_query_in_flight = false;
                    let available = result.unwrap_or(false);
                    if !available && session.borrow().active.is_some() && !session.borrow().busy {
                        elapsed.set_text("Session locked — pausing…");
                        pause_segment(&window, &elapsed, &pause, &stop, &discard, &session);
                    }
                },
            );
            glib::ControlFlow::Continue
        });
    }
}

fn begin_segment(
    window: &gtk::Window,
    status: &gtk::Label,
    pause: &gtk::Button,
    stop: &gtk::Button,
    discard: &gtk::Button,
    session: &Rc<RefCell<Session>>,
) {
    if session.borrow().busy {
        return;
    }
    if !captures_session::capture_session_available() {
        ui::error(
            window,
            "Screen capture is unavailable while the desktop session is locked.",
        );
        return;
    }
    let (options, display, path) = {
        let mut state = session.borrow_mut();
        state.busy = true;
        let path = state
            .work_directory
            .join(format!("native-segment-{:03}.mp4", state.segments.len()));
        (state.options.clone(), state.display.clone(), path)
    };
    status.set_text("Starting…");
    set_controls(pause, stop, discard, false);
    let (window, status, pause, stop, discard, session) = (
        window.clone(),
        status.clone(),
        pause.clone(),
        stop.clone(),
        discard.clone(),
        session.clone(),
    );
    ui::job(
        move || {
            if !captures_session::capture_session_available() {
                return Err(
                    "Screen capture is unavailable while the desktop session is locked.".into(),
                );
            }
            XcapRecordingSegment::start(&options, &path, &display).map_err(|e| e.to_string())
        },
        move |result| {
            let mut state = session.borrow_mut();
            state.busy = false;
            match result {
                Ok(segment) if !state.closing => {
                    state.active = Some(segment);
                    state.started = Instant::now();
                    if let Err(error) = register_active_segment(&mut state) {
                        let segment = state.active.take();
                        drop(state);
                        if let Some(segment) = segment {
                            discard_segment(segment);
                        }
                        ui::error(&window, &error);
                        discard_session(&window, &session);
                        return;
                    }
                    set_button_icon(&pause, "pause", "Pause recording");
                    set_controls(&pause, &stop, &discard, true);
                }
                Ok(segment) => {
                    drop(state);
                    discard_segment(segment);
                }
                Err(error) => {
                    drop(state);
                    ui::error(&window, &error);
                    discard_session(&window, &session);
                }
            }
            status.set_text("00:00");
        },
    );
}

fn pause_segment(
    window: &gtk::Window,
    status: &gtk::Label,
    pause: &gtk::Button,
    stop: &gtk::Button,
    discard: &gtk::Button,
    session: &Rc<RefCell<Session>>,
) {
    let Some(segment) = session.borrow_mut().active.take() else {
        return;
    };
    session.borrow_mut().busy = true;
    status.set_text("Pausing…");
    set_controls(pause, stop, discard, false);
    let (window, pause, stop, discard, session) = (
        window.clone(),
        pause.clone(),
        stop.clone(),
        discard.clone(),
        session.clone(),
    );
    ui::job(
        move || segment.stop().map_err(|e| e.to_string()),
        move |result| {
            let mut state = session.borrow_mut();
            state.busy = false;
            match result {
                Ok(info) => {
                    state.elapsed_before_segment += Duration::from_millis(info.duration_ms);
                    if let Err(error) = append_completed_segment(&mut state, info) {
                        drop(state);
                        fail_session_retain(&window, &session, &error);
                        return;
                    }
                    set_button_icon(&pause, "play", "Resume recording");
                    set_controls(&pause, &stop, &discard, true);
                }
                Err(error) => {
                    drop(state);
                    fail_session_retain(&window, &session, &error);
                }
            }
        },
    );
}

fn finish(
    window: &gtk::Window,
    pause: &gtk::Button,
    stop: &gtk::Button,
    discard: &gtk::Button,
    session: &Rc<RefCell<Session>>,
    on_saved: Rc<dyn Fn(PathBuf)>,
) {
    if session.borrow().busy {
        return;
    }
    let active = session.borrow_mut().active.take();
    session.borrow_mut().busy = true;
    set_controls(pause, stop, discard, false);
    let window_done = window.clone();
    let session_done = session.clone();
    let (pause_done, stop_done, discard_done) = (pause.clone(), stop.clone(), discard.clone());
    ui::job(
        move || match active {
            Some(segment) => segment.stop().map(Some).map_err(|e| e.to_string()),
            None => Ok(None),
        },
        move |stop_result| {
            let stopped = match stop_result {
                Ok(stopped) => stopped,
                Err(error) => {
                    session_done.borrow_mut().busy = false;
                    set_controls(&pause_done, &stop_done, &discard_done, true);
                    fail_session_retain(&window_done, &session_done, &error);
                    return;
                }
            };
            let (
                options,
                output_format,
                output_directory,
                work_directory,
                segments,
                draft_store,
                session_id,
            ) = {
                let mut state = session_done.borrow_mut();
                if let Some(info) = stopped {
                    state.elapsed_before_segment += Duration::from_millis(info.duration_ms);
                    if let Err(error) = append_completed_segment(&mut state, info) {
                        state.busy = false;
                        drop(state);
                        fail_session_retain(&window_done, &session_done, &error);
                        return;
                    }
                }
                state.manifest.state = RecordingState::Finalizing;
                state.manifest.updated_at_ms = now_ms();
                if let Err(error) = state.draft_store.save(&state.manifest) {
                    state.busy = false;
                    drop(state);
                    set_controls(&pause_done, &stop_done, &discard_done, true);
                    ui::error(&window_done, &error.to_string());
                    return;
                }
                (
                    state.options.clone(),
                    state.output_format,
                    state.output_directory.clone(),
                    state.work_directory.clone(),
                    state.segments.clone(),
                    state.draft_store.clone(),
                    state.manifest.session_id.clone(),
                )
            };
            let (window, session, pause, stop, discard) = (
                window_done.clone(),
                session_done.clone(),
                pause_done.clone(),
                stop_done.clone(),
                discard_done.clone(),
            );
            let work_for_export = work_directory.clone();
            ui::job(
                move || {
                    if segments.is_empty() {
                        return Err("The recording contains no media.".into());
                    }
                    let extension = if output_format == ExportFormat::Gif {
                        "gif"
                    } else {
                        "mp4"
                    };
                    let destination = unique_output(&output_directory, extension)?;
                    let media = MediaToolchain::from_command_names();
                    media.verify().map_err(|e| e.to_string())?;
                    let inputs = segment_inputs(&segments);
                    let cancel = CancelToken::default();
                    if output_format == ExportFormat::Gif {
                        let master = work_for_export.join("gif-master.mp4");
                        media
                            .concatenate_segments(
                                &inputs
                                    .iter()
                                    .map(|i| i.video_path.clone())
                                    .collect::<Vec<_>>(),
                                &master,
                                &cancel,
                            )
                            .map_err(|e| e.to_string())?;
                        let result = media
                            .create_gif(
                                &master,
                                &destination.path,
                                options.frames_per_second,
                                options.gif.max_width,
                                options.gif.max_colors,
                                &cancel,
                            )
                            .map_err(|e| e.to_string());
                        let _ = fs::remove_file(master);
                        result?;
                    } else {
                        media
                            .assemble_recording_segments(
                                &inputs,
                                &destination.path,
                                RecordingAudioLayout {
                                    system_audio: options.audio.capture_system_audio,
                                    microphone_audio: segments
                                        .iter()
                                        .any(|s| s.microphone_path.is_some()),
                                },
                                &cancel,
                            )
                            .map_err(|e| e.to_string())?;
                    }
                    let published = destination.commit()?;
                    draft_store
                        .remove(&session_id)
                        .map_err(|error| error.to_string())?;
                    Ok(published)
                },
                move |result| match result {
                    Ok(path) => {
                        let mut state = session.borrow_mut();
                        state.closing = true;
                        if let Some(border) = state.region_border.take() {
                            border.close();
                        }
                        drop(state);
                        notify_finished(&session);
                        window.close();
                        on_saved(path);
                    }
                    Err(error) => {
                        let mut state = session.borrow_mut();
                        state.busy = false;
                        state.manifest.state = RecordingState::Failed;
                        state.manifest.last_error = Some(error.clone());
                        state.manifest.updated_at_ms = now_ms();
                        let _ = state.draft_store.save(&state.manifest);
                        drop(state);
                        set_controls(&pause, &stop, &discard, true);
                        ui::error(
                            &window,
                            &format!(
                                "{error}\n\nThe recoverable draft was retained at {}.",
                                work_directory.display()
                            ),
                        );
                    }
                },
            );
        },
    );
}

fn discard_session(window: &gtk::Window, session: &Rc<RefCell<Session>>) {
    let (active, draft_store, session_id) = {
        let mut state = session.borrow_mut();
        if state.closing || state.busy {
            return;
        }
        state.closing = true;
        if let Some(border) = state.region_border.take() {
            border.close();
        }
        (
            state.active.take(),
            state.draft_store.clone(),
            state.manifest.session_id.clone(),
        )
    };
    window.hide();
    let window = window.clone();
    let session = session.clone();
    ui::job(
        move || {
            if let Some(segment) = active {
                segment.discard().map_err(|e| e.to_string())?;
            }
            draft_store
                .remove(&session_id)
                .map_err(|error| error.to_string())?;
            Ok(())
        },
        move |result| {
            if let Err(error) = result {
                ui::error(&window, &error);
            }
            notify_finished(&session);
            window.close();
        },
    );
}

fn fail_session_retain(window: &gtk::Window, session: &Rc<RefCell<Session>>, error: &str) {
    let draft = {
        let mut state = session.borrow_mut();
        state.busy = false;
        state.closing = true;
        state.manifest.state = RecordingState::Failed;
        state.manifest.updated_at_ms = now_ms();
        state.manifest.last_error = Some(error.to_owned());
        let _ = state.draft_store.save(&state.manifest);
        if let Some(border) = state.region_border.take() {
            border.close();
        }
        state.work_directory.clone()
    };
    notify_finished(session);
    ui::error(
        window,
        &format!(
            "{error}\n\nThe recoverable recording draft was retained at {}.",
            draft.display()
        ),
    );
    window.close();
}

fn discard_segment(segment: XcapRecordingSegment) {
    std::thread::spawn(move || {
        let _ = segment.discard();
    });
}

fn notify_finished(session: &Rc<RefCell<Session>>) {
    let on_finished = session.borrow_mut().on_finished.take();
    if let Some(on_finished) = on_finished {
        on_finished();
    }
}

#[allow(clippy::too_many_arguments)]
fn begin_screenshot_action(
    window: &gtk::Window,
    status: &gtk::Label,
    pause: &gtk::Button,
    stop: &gtk::Button,
    discard: &gtk::Button,
    screenshot: &gtk::Button,
    session: &Rc<RefCell<Session>>,
    action: ScreenshotAction,
) {
    let resume_after = {
        let mut state = session.borrow_mut();
        if state.busy || state.closing || state.screenshot_in_flight {
            return;
        }
        state.screenshot_in_flight = true;
        state.active.is_some()
    };
    screenshot.set_sensitive(false);
    if resume_after {
        pause_segment(window, status, pause, stop, discard, session);
    }
    let (window, status, pause, stop, discard, screenshot, session) = (
        window.clone(),
        status.clone(),
        pause.clone(),
        stop.clone(),
        discard.clone(),
        screenshot.clone(),
        session.clone(),
    );
    glib::timeout_add_local(Duration::from_millis(30), move || {
        let state = session.borrow();
        if state.closing {
            return glib::ControlFlow::Break;
        }
        if state.busy {
            return glib::ControlFlow::Continue;
        }
        drop(state);
        window.hide();
        if let Some(border) = session.borrow_mut().region_border.take() {
            border.close();
        }
        let completed = Rc::new(Cell::new(false));
        let done: Rc<dyn Fn()> = Rc::new({
            let (window, status, pause, stop, discard, screenshot, session, completed) = (
                window.clone(),
                status.clone(),
                pause.clone(),
                stop.clone(),
                discard.clone(),
                screenshot.clone(),
                session.clone(),
                completed.clone(),
            );
            move || {
                if completed.replace(true) {
                    return;
                }
                let (target, display, include_controls) = {
                    let mut state = session.borrow_mut();
                    state.screenshot_in_flight = false;
                    if state.closing {
                        return;
                    }
                    (
                        state.options.target.clone(),
                        state.display.clone(),
                        state.include_controls,
                    )
                };
                session.borrow_mut().region_border =
                    RegionBorder::for_target(&target, &display, include_controls);
                window.show_all();
                screenshot.set_sensitive(true);
                if resume_after {
                    begin_segment(&window, &status, &pause, &stop, &discard, &session);
                }
            }
        });
        action(done);
        glib::ControlFlow::Break
    });
}

fn icon_button(icon_name: &str, accessible_name: &str) -> gtk::Button {
    let button = ui::icon_button(accessible_name, icon_name);
    button.set_size_request(34, 34);
    button
}

fn set_button_icon(button: &gtk::Button, icon_name: &str, accessible_name: &str) {
    button.set_image(Some(&ui::icon(icon_name, 16)));
    button.set_always_show_image(true);
    button.set_tooltip_text(Some(accessible_name));
    if let Some(accessible) = button.accessible() {
        accessible.set_name(accessible_name);
    }
}

fn install_hud_transparency() {
    let provider = gtk::CssProvider::new();
    if provider
        .load_from_data(
            br#"
            window.recording-hud-window, window.recording-hud-window.background {
                background-color: transparent;
            }
            window.recording-hud-window .glass {
                background-color: @captures_glass;
                color: @captures_glass_text;
                border: 1px solid alpha(@captures_glass_text, 0.16);
                border-radius: 18px;
            }
            window.recording-hud-window .recording-hud-main {
                min-height: 38px;
            }
            window.recording-hud-window .recording-hud-status .title {
                font-size: 15px;
                font-weight: 600;
                padding: 0;
            }
            window.recording-hud-window .recording-hud-status .muted {
                font-size: 9px;
                font-weight: 600;
                letter-spacing: 0.7px;
            }
            window.recording-hud-window .recording-dot {
                color: @captures_signal;
                font-size: 16px;
            }
            window.recording-hud-window .recording-hud-actions button {
                min-width: 34px;
                min-height: 34px;
                padding: 0;
                border-color: transparent;
                background-color: transparent;
            }
            window.recording-hud-window .recording-hud-actions button:hover {
                background-color: alpha(@captures_glass_text, 0.10);
            }
            window.recording-hud-window .recording-hud-actions button.recording-stop {
                min-width: 36px;
                border-color: alpha(@captures_signal, 0.4);
                background-color: alpha(@captures_signal, 0.14);
            }
            window.recording-hud-window .recording-microphone-level {
                min-width: 34px;
                min-height: 4px;
                border-radius: 2px;
                background-color: alpha(black, 0.4);
            }
            "#,
        )
        .is_ok()
        && let Some(screen) = gtk::gdk::Screen::default()
    {
        gtk::StyleContext::add_provider_for_screen(
            &screen,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );
    }
}

fn toggle_microphone(
    window: &gtk::Window,
    status: &gtk::Label,
    pause: &gtk::Button,
    stop: &gtk::Button,
    discard: &gtk::Button,
    microphone: &gtk::Button,
    session: &Rc<RefCell<Session>>,
) {
    if session.borrow().busy
        || session
            .borrow()
            .options
            .audio
            .microphone_device_id
            .is_none()
    {
        return;
    }
    let muted = !session.borrow().options.audio.microphone_muted;
    {
        let mut state = session.borrow_mut();
        state.options.audio.microphone_muted = muted;
        state.manifest.options.audio.microphone_muted = muted;
        state.manifest.updated_at_ms = now_ms();
        if let Err(error) = state.draft_store.save(&state.manifest) {
            ui::error(window, &error.to_string());
            return;
        }
    }
    set_button_icon(
        microphone,
        if muted {
            "microphone-off"
        } else {
            "microphone"
        },
        if muted {
            "Unmute microphone"
        } else {
            "Mute microphone"
        },
    );
    if session.borrow().active.is_none() {
        begin_segment(window, status, pause, stop, discard, session);
        return;
    }
    pause_segment(window, status, pause, stop, discard, session);
    let (window, status, pause, stop, discard, session) = (
        window.clone(),
        status.clone(),
        pause.clone(),
        stop.clone(),
        discard.clone(),
        session.clone(),
    );
    glib::timeout_add_local(Duration::from_millis(30), move || {
        let state = session.borrow();
        if state.closing {
            return glib::ControlFlow::Break;
        }
        if state.busy {
            return glib::ControlFlow::Continue;
        }
        drop(state);
        begin_segment(&window, &status, &pause, &stop, &discard, &session);
        glib::ControlFlow::Break
    });
}

fn restart_session(
    window: &gtk::Window,
    status: &gtk::Label,
    pause: &gtk::Button,
    stop: &gtk::Button,
    discard: &gtk::Button,
    session: &Rc<RefCell<Session>>,
) {
    if session.borrow().busy {
        return;
    }
    let (active, store, session_id, directory, options, output_format) = {
        let mut state = session.borrow_mut();
        state.busy = true;
        (
            state.active.take(),
            state.draft_store.clone(),
            state.manifest.session_id.clone(),
            state.output_directory.clone(),
            state.options.clone(),
            state.output_format,
        )
    };
    status.set_text("Restarting…");
    set_controls(pause, stop, discard, false);
    let (window, status, pause, stop, discard, session) = (
        window.clone(),
        status.clone(),
        pause.clone(),
        stop.clone(),
        discard.clone(),
        session.clone(),
    );
    ui::job(
        move || {
            if let Some(segment) = active {
                segment.discard().map_err(|error| error.to_string())?;
            }
            store
                .remove(&session_id)
                .map_err(|error| error.to_string())?;
            create_recording_draft(
                &directory,
                &recording_manifest_options(&options, output_format),
            )
        },
        move |result| match result {
            Ok((store, manifest, work_directory)) => {
                let mut state = session.borrow_mut();
                state.draft_store = store;
                state.manifest = manifest;
                state.work_directory = work_directory;
                state.segments.clear();
                state.elapsed_before_segment = Duration::ZERO;
                state.busy = false;
                drop(state);
                begin_segment(&window, &status, &pause, &stop, &discard, &session);
            }
            Err(error) => {
                let mut state = session.borrow_mut();
                state.busy = false;
                state.closing = true;
                if let Some(border) = state.region_border.take() {
                    border.close();
                }
                drop(state);
                notify_finished(&session);
                ui::error(&window, &error);
                window.close();
            }
        },
    );
}

fn set_controls(pause: &gtk::Button, stop: &gtk::Button, discard: &gtk::Button, enabled: bool) {
    pause.set_sensitive(enabled);
    stop.set_sensitive(enabled);
    discard.set_sensitive(enabled);
}
fn format_duration(value: Duration) -> String {
    let seconds = value.as_secs();
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn recording_output_format(
    options: &RecordingOptions,
    preferred_video_format: ExportFormat,
) -> Result<ExportFormat, String> {
    if options.kind == RecordingKind::Gif {
        return Ok(ExportFormat::Gif);
    }
    match preferred_video_format {
        ExportFormat::Mp4 | ExportFormat::Gif => Ok(preferred_video_format),
        ExportFormat::WebM => Err(
            "WebM recording is unavailable in the shipping Linux media backend. Choose MP4 or GIF."
                .into(),
        ),
    }
}

fn recording_manifest_options(
    options: &RecordingOptions,
    output_format: ExportFormat,
) -> RecordingOptions {
    let mut manifest_options = options.clone();
    if output_format == ExportFormat::Gif {
        manifest_options.kind = RecordingKind::Gif;
    }
    manifest_options
}

fn segment_inputs(segments: &[RecordingSegmentInfo]) -> Vec<RecordingSegmentInput> {
    segments
        .iter()
        .map(|s| RecordingSegmentInput {
            video_path: s.path.clone(),
            system_audio_path: s.system_audio_path.clone(),
            system_audio_offset_ms: s.system_audio_offset_ms,
            microphone_path: s.microphone_path.clone(),
            microphone_offset_ms: s.microphone_offset_ms,
            duration_ms: s.duration_ms,
        })
        .collect()
}

/// List retained recording sessions for app startup/history recovery UI.
pub fn list_recording_drafts(
    output_directory: &Path,
) -> Result<Vec<RecordingDraftManifest>, String> {
    recording_draft_store(output_directory)
        .list()
        .map_err(|error| error.to_string())
}

/// Assemble the completed segments in a retained session and safely publish a new file.
///
/// Incomplete trailing segments are ignored because their container may not have been finalized.
/// A failed recovery updates and retains the manifest so the source media is never discarded.
pub fn recover_recording_draft(
    output_directory: &Path,
    session_id: &str,
) -> Result<PathBuf, String> {
    let store = recording_draft_store(output_directory);
    let mut manifest = store.load(session_id).map_err(|error| error.to_string())?;
    let result = (|| {
        if manifest.schema_version != 1 {
            return Err(format!(
                "Recording draft schema {} is not supported",
                manifest.schema_version
            ));
        }
        let session_directory = store
            .session_directory(session_id)
            .map_err(|error| error.to_string())?;
        let mut saw_incomplete = false;
        for segment in &manifest.segments {
            if segment.complete && saw_incomplete {
                return Err(
                    "The recording draft contains completed media after an incomplete segment."
                        .into(),
                );
            }
            saw_incomplete |= !segment.complete;
        }
        let mut completed = manifest
            .segments
            .iter()
            .filter(|segment| segment.complete)
            .collect::<Vec<_>>();
        completed.sort_by_key(|segment| segment.index);
        if completed.is_empty() {
            return Err("The recording draft contains no completed media segments.".into());
        }
        if completed
            .iter()
            .enumerate()
            .any(|(expected, segment)| segment.index != expected as u32)
        {
            return Err("The recording draft segment indexes are not contiguous.".into());
        }
        let inputs = completed
            .into_iter()
            .map(|segment| {
                Ok(RecordingSegmentInput {
                    video_path: checked_draft_media_path(
                        &session_directory,
                        &segment.relative_path,
                    )?,
                    system_audio_path: segment
                        .system_audio_relative_path
                        .as_deref()
                        .map(|path| checked_draft_media_path(&session_directory, path))
                        .transpose()?,
                    system_audio_offset_ms: segment.system_audio_offset_ms,
                    microphone_path: segment
                        .microphone_relative_path
                        .as_deref()
                        .map(|path| checked_draft_media_path(&session_directory, path))
                        .transpose()?,
                    microphone_offset_ms: segment.microphone_offset_ms,
                    duration_ms: segment.duration_ms,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let extension = if manifest.options.kind == RecordingKind::Gif {
            "gif"
        } else {
            "mp4"
        };
        let destination = unique_output(output_directory, extension)?;
        let media = MediaToolchain::from_command_names();
        media.verify().map_err(|error| error.to_string())?;
        let cancel = CancelToken::default();
        if manifest.options.kind == RecordingKind::Gif {
            let master = destination.work.join("recovered-master.mp4");
            media
                .concatenate_segments(
                    &inputs
                        .iter()
                        .map(|input| input.video_path.clone())
                        .collect::<Vec<_>>(),
                    &master,
                    &cancel,
                )
                .map_err(|error| error.to_string())?;
            media
                .create_gif(
                    &master,
                    &destination.path,
                    manifest.options.frames_per_second,
                    manifest.options.gif.max_width,
                    manifest.options.gif.max_colors,
                    &cancel,
                )
                .map_err(|error| error.to_string())?;
        } else {
            media
                .assemble_recording_segments(
                    &inputs,
                    &destination.path,
                    RecordingAudioLayout {
                        system_audio: inputs.iter().any(|input| input.system_audio_path.is_some()),
                        microphone_audio: inputs
                            .iter()
                            .any(|input| input.microphone_path.is_some()),
                    },
                    &cancel,
                )
                .map_err(|error| error.to_string())?;
        }
        destination.commit()
    })();
    match result {
        Ok(path) => {
            store
                .remove(session_id)
                .map_err(|error| error.to_string())?;
            Ok(path)
        }
        Err(error) => {
            manifest.state = RecordingState::Failed;
            manifest.updated_at_ms = now_ms();
            manifest.last_error = Some(error.clone());
            let _ = store.save(&manifest);
            Err(error)
        }
    }
}

/// Explicitly remove one retained recording and all of its private source media.
pub fn discard_recording_draft(output_directory: &Path, session_id: &str) -> Result<(), String> {
    recording_draft_store(output_directory)
        .remove(session_id)
        .map_err(|error| error.to_string())
}

fn recording_draft_store(output_directory: &Path) -> DraftStore {
    DraftStore::new(output_directory.join(".captures-recording-drafts"))
}

fn checked_draft_media_path(directory: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative.components().any(|component| {
            !matches!(
                component,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
    {
        return Err("Recording draft media path escapes its session directory.".into());
    }
    let path = directory.join(relative);
    let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("Recording draft media must be a regular private file.".into());
    }
    Ok(path)
}

fn create_recording_draft(
    output_directory: &Path,
    options: &RecordingOptions,
) -> Result<(DraftStore, RecordingDraftManifest, PathBuf), String> {
    let root = output_directory.join(".captures-recording-drafts");
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(&root).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    fs::set_permissions(&root, std::os::unix::fs::PermissionsExt::from_mode(0o700))
        .map_err(|error| error.to_string())?;
    #[cfg(target_os = "windows")]
    crate::windows::private_directory(&root).map_err(|error| error.to_string())?;
    let store = DraftStore::new(root);
    let manifest = RecordingDraftManifest::new(session_id(), options.clone(), now_ms());
    let directory = store.create(&manifest).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    fs::set_permissions(
        &directory,
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .map_err(|error| error.to_string())?;
    Ok((store, manifest, directory))
}

fn register_active_segment(state: &mut Session) -> Result<(), String> {
    let segment = state
        .active
        .as_ref()
        .ok_or("recording segment is missing")?;
    let index = state.manifest.segments.len() as u32;
    let relative_path = relative_draft_path(
        &state.work_directory,
        &state
            .work_directory
            .join(format!("native-segment-{index:03}.mp4")),
    )?;
    let (system_audio_relative_path, system_audio_offset_ms) =
        if let Some((path, offset)) = segment.system_audio_draft_info() {
            (
                Some(relative_draft_path(&state.work_directory, &path)?),
                offset,
            )
        } else {
            (None, 0)
        };
    let (microphone_relative_path, microphone_offset_ms) =
        if let Some((path, offset)) = segment.microphone_draft_info() {
            (
                Some(relative_draft_path(&state.work_directory, &path)?),
                offset,
            )
        } else {
            (None, 0)
        };
    let (width, height) = segment.dimensions();
    state.manifest.segments.push(RecordingSegmentManifest {
        index,
        relative_path,
        system_audio_relative_path,
        system_audio_offset_ms,
        system_audio_warning: None,
        microphone_relative_path,
        microphone_offset_ms,
        microphone_warning: None,
        started_at_ms: now_ms(),
        duration_ms: 0,
        width,
        height,
        size_bytes: 0,
        dropped_frames: 0,
        complete: false,
    });
    state.manifest.state = RecordingState::Recording;
    state.manifest.updated_at_ms = now_ms();
    state
        .draft_store
        .save(&state.manifest)
        .map_err(|error| error.to_string())
}

fn append_completed_segment(state: &mut Session, info: RecordingSegmentInfo) -> Result<(), String> {
    let relative = relative_draft_path(&state.work_directory, &info.path)?;
    let pending = state
        .manifest
        .segments
        .iter_mut()
        .rev()
        .find(|segment| !segment.complete && segment.relative_path == relative)
        .ok_or("recording draft did not contain the active segment")?;
    pending.system_audio_relative_path = info
        .system_audio_path
        .as_ref()
        .map(|path| relative_draft_path(&state.work_directory, path))
        .transpose()?;
    pending.system_audio_offset_ms = info.system_audio_offset_ms;
    pending.system_audio_warning = info.system_audio_warning.clone();
    pending.microphone_relative_path = info
        .microphone_path
        .as_ref()
        .map(|path| relative_draft_path(&state.work_directory, path))
        .transpose()?;
    pending.microphone_offset_ms = info.microphone_offset_ms;
    pending.microphone_warning = info.microphone_warning.clone();
    pending.duration_ms = info.duration_ms;
    pending.width = info.width;
    pending.height = info.height;
    pending.size_bytes = info.size_bytes;
    pending.dropped_frames = info.dropped_frames;
    pending.complete = true;
    state.segments.push(info);
    state.manifest.state = RecordingState::Paused;
    state.manifest.updated_at_ms = now_ms();
    state
        .draft_store
        .save(&state.manifest)
        .map_err(|error| error.to_string())
}

fn relative_draft_path(directory: &Path, path: &Path) -> Result<String, String> {
    path.strip_prefix(directory)
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|_| "recording media escaped its private draft directory".to_owned())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn session_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let value = u128::from(now_ms()) << 32
        | u128::from(std::process::id()) << 16
        | u128::from(NEXT.fetch_add(1, Ordering::Relaxed));
    format!(
        "{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}",
        (value >> 96) as u32,
        ((value >> 80) & 0xffff) as u16,
        ((value >> 68) & 0x0fff) as u16,
        ((value >> 52) & 0x0fff) as u16,
        value & 0xffffffffffff
    )
}

struct RegionBorder {
    windows: Vec<gtk::Window>,
}

impl RegionBorder {
    fn for_target(
        target: &RecordingTarget,
        display: &DisplayDescriptor,
        include_controls: bool,
    ) -> Option<Self> {
        let RecordingTarget::Region { rect, .. } = target else {
            return None;
        };
        let thickness = 3_i32;
        let width = i32::try_from(rect.width).ok()?;
        let height = i32::try_from(rect.height).ok()?;
        let geometries = [
            (rect.x, rect.y, width, thickness),
            (rect.x, rect.y + height - thickness, width, thickness),
            (rect.x, rect.y, thickness, height),
            (rect.x + width - thickness, rect.y, thickness, height),
        ];
        let windows = geometries
            .into_iter()
            .map(|(x, y, width, height)| {
                let window = gtk::Window::new(gtk::WindowType::Popup);
                window.set_decorated(false);
                window.set_keep_above(true);
                window.set_accept_focus(false);
                window.set_skip_taskbar_hint(true);
                window.set_type_hint(gtk::gdk::WindowTypeHint::Notification);
                let color = ui::color("signal");
                let fill = gtk::DrawingArea::new();
                fill.connect_draw(move |area, context| {
                    context.set_source_rgba(color.red(), color.green(), color.blue(), 0.95);
                    context.rectangle(
                        0.0,
                        0.0,
                        f64::from(area.allocated_width()),
                        f64::from(area.allocated_height()),
                    );
                    let _ = context.fill();
                    glib::Propagation::Proceed
                });
                window.add(&fill);
                window.move_(x, y);
                window.resize(width.max(1), height.max(1));
                #[cfg(target_os = "windows")]
                if let Err(error) = crate::windows::exclude_from_capture(&window, !include_controls)
                {
                    eprintln!("Recording guide hidden: {error}");
                    return window;
                }
                window.show_all();
                #[cfg(target_os = "windows")]
                {
                    let scale = display.scale_factor.max(1.);
                    if let Err(error) = crate::windows::place_window(
                        &window,
                        display.x + (f64::from(x) * scale).round() as i32,
                        display.y + (f64::from(y) * scale).round() as i32,
                        (f64::from(width) * scale).round() as i32,
                        (f64::from(height) * scale).round() as i32,
                    ) {
                        window.hide();
                        eprintln!("Recording guide hidden: {error}");
                    }
                }
                window
            })
            .collect();
        #[cfg(not(target_os = "windows"))]
        let _ = (display, include_controls);
        Some(Self { windows })
    }

    fn close(self) {
        for window in self.windows {
            window.close();
        }
    }
}
struct Output {
    path: PathBuf,
    work: PathBuf,
    directory: PathBuf,
    extension: String,
    stem: Option<String>,
}
impl Output {
    fn commit(&self) -> Result<PathBuf, String> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis();
        for n in 0.. {
            let destination = self.directory.join(if let Some(stem) = &self.stem {
                format!(
                    "{stem}{}.{}",
                    if n == 0 {
                        String::new()
                    } else {
                        format!(" {n}")
                    },
                    self.extension
                )
            } else {
                format!("Capture-{stamp}-{n}.{}", self.extension)
            });
            // Same-filesystem hard linking publishes atomically and never replaces a file.
            match fs::hard_link(&self.path, &destination) {
                Ok(()) => return Ok(destination),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        unreachable!()
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.work);
    }
}
fn unique_output(directory: &Path, extension: &str) -> Result<Output, String> {
    let work = create_private_work_directory(directory, "export")?;
    Ok(Output {
        path: work.join(format!("media.{extension}")),
        work,
        directory: directory.to_owned(),
        extension: extension.to_owned(),
        stem: None,
    })
}

fn named_output(directory: &Path, stem: &str, extension: &str) -> Result<Output, String> {
    let stem = stem.trim();
    if stem.is_empty()
        || stem == "."
        || stem == ".."
        || stem.chars().any(|character| {
            character.is_control()
                || matches!(
                    character,
                    '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
                )
        })
    {
        return Err("Enter a filename without path or reserved characters.".into());
    }
    let work = create_private_work_directory(directory, "export")?;
    Ok(Output {
        path: work.join(format!("media.{extension}")),
        work,
        directory: directory.to_owned(),
        extension: extension.to_owned(),
        stem: Some(stem.to_owned()),
    })
}

fn create_private_work_directory(parent: &Path, purpose: &str) -> Result<PathBuf, String> {
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    for n in 0.. {
        let path = parent.join(format!(".captures-{purpose}-{stamp}-{n}"));
        let builder = fs::DirBuilder::new();
        #[cfg(unix)]
        let mut builder = builder;
        #[cfg(unix)]
        builder.mode(0o700);
        match builder.create(&path) {
            Ok(()) => {
                #[cfg(target_os = "windows")]
                if let Err(error) = crate::windows::private_directory(&path) {
                    let _ = fs::remove_dir(&path);
                    return Err(error.to_string());
                }
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    unreachable!()
}

/// Open the visual native editor (never edits the source).
pub fn open_editor(path: PathBuf, directory: PathBuf, on_saved: Rc<dyn Fn(PathBuf)>) {
    editor::open(path, directory, on_saved);
}

// Kept temporarily as an implementation comparison while the native experiment
// is reviewed. The visual editor above owns the public path.
#[allow(dead_code)]
fn open_editor_legacy(path: PathBuf, directory: PathBuf, on_saved: Rc<dyn Fn(PathBuf)>) {
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("Edit recording — Captures");
    window.set_default_size(760, 650);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_border_width(16);
    let preview = gtk::Image::new();
    preview.set_size_request(640, 300);
    let status = ui::label("Reading recording…", "muted");
    let scrub = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 1.0);
    scrub.set_draw_value(false);
    root.pack_start(&preview, true, true, 0);
    root.pack_start(&scrub, false, false, 0);
    let grid = gtk::Grid::new();
    grid.set_row_spacing(8);
    grid.set_column_spacing(8);
    let trim_start = spin(0.0, 86_400_000.0, 100.0);
    let trim_end = spin(0.0, 86_400_000.0, 100.0);
    let crop_x = spin(0.0, 20_000.0, 1.0);
    let crop_y = spin(0.0, 20_000.0, 1.0);
    let crop_w = spin(0.0, 20_000.0, 1.0);
    let crop_h = spin(0.0, 20_000.0, 1.0);
    let out_w = spin(0.0, 20_000.0, 2.0);
    let out_h = spin(0.0, 20_000.0, 2.0);
    let volume = spin(0.0, 200.0, 5.0);
    volume.set_value(100.0);
    let mute = gtk::CheckButton::with_label("Mute audio");
    let fields = [
        ("Trim start (ms)", &trim_start),
        ("Trim end (ms)", &trim_end),
        ("Crop X", &crop_x),
        ("Crop Y", &crop_y),
        ("Crop width", &crop_w),
        ("Crop height", &crop_h),
        ("Output width", &out_w),
        ("Output height", &out_h),
        ("Audio volume %", &volume),
    ];
    for (index, (name, field)) in fields.iter().enumerate() {
        let column = (index % 3) as i32 * 2;
        let row = (index / 3) as i32;
        grid.attach(&ui::label(name, "muted"), column, row, 1, 1);
        grid.attach(*field, column + 1, row, 1, 1);
    }
    grid.attach(&mute, 0, 3, 2, 1);
    root.pack_start(&grid, false, false, 0);
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let play = ui::button("Play in system player");
    let mp4 = ui::button("Export MP4");
    let gif = ui::button("Export GIF");
    let webm = ui::button("Export WebM");
    webm.set_sensitive(false);
    webm.set_tooltip_text(Some(
        "WebM export is not supported by the native media toolchain.",
    ));
    let cancel = ui::button("Cancel export");
    for button in [&play, &mp4, &gif, &webm, &cancel] {
        actions.pack_start(button, false, false, 0);
    }
    root.pack_start(&status, false, false, 0);
    root.pack_start(&actions, false, false, 0);
    window.add(&root);
    window.show_all();
    cancel.set_sensitive(false);

    let preview_directory = match create_private_work_directory(&std::env::temp_dir(), "editor") {
        Ok(path) => path,
        Err(error) => {
            ui::error(&window, &error);
            window.close();
            return;
        }
    };
    let editor_closed = Rc::new(Cell::new(false));

    let duration = Rc::new(Cell::new(0_u64));
    let probe_path = path.clone();
    let status_probe = status.clone();
    let scrub_probe = scrub.clone();
    let duration_probe = duration.clone();
    let window_probe = window.clone();
    let trim_end_probe = trim_end.clone();
    let crop_w_probe = crop_w.clone();
    let crop_h_probe = crop_h.clone();
    let out_w_probe = out_w.clone();
    let out_h_probe = out_h.clone();
    let editor_closed_probe = editor_closed.clone();
    ui::job(
        move || {
            MediaToolchain::from_command_names()
                .probe(&probe_path)
                .map_err(|e| e.to_string())
        },
        move |result| match result {
            Ok(probe) => {
                let ms = probe.metadata.duration_ms.unwrap_or(0);
                duration_probe.set(ms);
                scrub_probe.set_range(0.0, ms as f64);
                trim_end_probe.set_value(ms as f64);
                crop_w_probe.set_value(probe.metadata.width as f64);
                crop_h_probe.set_value(probe.metadata.height as f64);
                out_w_probe.set_value(probe.metadata.width as f64);
                out_h_probe.set_value(probe.metadata.height as f64);
                status_probe.set_text(&format!(
                    "{}×{} • {}",
                    probe.metadata.width,
                    probe.metadata.height,
                    format_duration(Duration::from_millis(ms))
                ));
            }
            Err(error) if !editor_closed_probe.get() => ui::error(&window_probe, &error),
            Err(_) => {}
        },
    );
    let preview_generation = Rc::new(Cell::new(0_u64));
    let preview_cancel = Rc::new(RefCell::new(None::<CancelToken>));
    {
        let (path, preview, generation, duration, preview_cancel, preview_directory, editor_closed) = (
            path.clone(),
            preview.clone(),
            preview_generation.clone(),
            duration.clone(),
            preview_cancel.clone(),
            preview_directory.clone(),
            editor_closed.clone(),
        );
        scrub.connect_value_changed(move |scale| {
            let generation_value = generation.get().wrapping_add(1);
            generation.set(generation_value);
            let requested_at = scale.value().max(0.0) as u64;
            if let Some(token) = preview_cancel.borrow_mut().take() {
                token.cancel();
            }
            let (
                path,
                preview,
                generation,
                duration,
                preview_cancel,
                preview_directory,
                editor_closed,
            ) = (
                path.clone(),
                preview.clone(),
                generation.clone(),
                duration.clone(),
                preview_cancel.clone(),
                preview_directory.clone(),
                editor_closed.clone(),
            );
            glib::timeout_add_local_once(Duration::from_millis(120), move || {
                if editor_closed.get() || generation.get() != generation_value {
                    return;
                }
                let token = CancelToken::default();
                *preview_cancel.borrow_mut() = Some(token.clone());
                let temp = preview_directory.join(format!("frame-{generation_value}.png"));
                let end = duration.get();
                let at = requested_at.min(end.saturating_sub(1));
                let temp_work = temp.clone();
                let (preview, generation, preview_cancel, editor_closed) = (
                    preview.clone(),
                    generation.clone(),
                    preview_cancel.clone(),
                    editor_closed.clone(),
                );
                ui::job(
                    move || {
                        MediaToolchain::from_command_names()
                            .extract_frame(&path, at, &temp_work, &token)
                            .map_err(|e| e.to_string())?;
                        Ok(temp_work)
                    },
                    move |result| {
                        *preview_cancel.borrow_mut() = None;
                        if let Ok(file) = result {
                            if !editor_closed.get()
                                && generation.get() == generation_value
                                && let Ok(pixbuf) = gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(
                                    &file, 640, 300, true,
                                )
                            {
                                preview.set_from_pixbuf(Some(&pixbuf));
                            }
                            let _ = fs::remove_file(file);
                        }
                    },
                );
            });
        });
        scrub.set_value(1.0);
    }
    {
        let path = path.clone();
        let window = window.clone();
        play.connect_clicked(move |_| {
            if let Err(error) = gtk::show_uri_on_window(
                Some(&window),
                &glib::filename_to_uri(&path, None).unwrap_or_default(),
                0,
            ) {
                ui::error(&window, &error.to_string());
            }
        });
    }
    let active_cancel = Rc::new(RefCell::new(None::<CancelToken>));
    for (button, format) in [(mp4, ExportFormat::Mp4), (gif, ExportFormat::Gif)] {
        let (
            path,
            directory,
            window,
            status,
            cancel_button,
            active_cancel,
            on_saved,
            editor_closed,
        ) = (
            path.clone(),
            directory.clone(),
            window.clone(),
            status.clone(),
            cancel.clone(),
            active_cancel.clone(),
            on_saved.clone(),
            editor_closed.clone(),
        );
        let (trim_start, trim_end, crop_x, crop_y, crop_w, crop_h, out_w, out_h, volume, mute) = (
            trim_start.clone(),
            trim_end.clone(),
            crop_x.clone(),
            crop_y.clone(),
            crop_w.clone(),
            crop_h.clone(),
            out_w.clone(),
            out_h.clone(),
            volume.clone(),
            mute.clone(),
        );
        button.connect_clicked(move |_| {
            if active_cancel.borrow().is_some() {
                return;
            }
            let edit = EditSpec {
                trim_start_ms: trim_start.value_as_int() as u64,
                trim_end_ms: Some(trim_end.value_as_int() as u64),
                crop: (crop_w.value_as_int() > 0 && crop_h.value_as_int() > 0).then(|| CropRect {
                    x: crop_x.value_as_int() as u32,
                    y: crop_y.value_as_int() as u32,
                    width: crop_w.value_as_int() as u32,
                    height: crop_h.value_as_int() as u32,
                }),
                output_width: (out_w.value_as_int() > 0).then(|| out_w.value_as_int() as u32),
                output_height: (out_h.value_as_int() > 0).then(|| out_h.value_as_int() as u32),
                audio: AudioEdit {
                    system_volume: (volume.value() / 100.) as f32,
                    microphone_volume: (volume.value() / 100.) as f32,
                    mute_system_audio: mute.is_active(),
                    mute_microphone: mute.is_active(),
                    mono_output: false,
                    source_has_system_audio: true,
                    source_has_microphone_audio: false,
                },
            };
            let extension = match format {
                ExportFormat::Mp4 => "mp4",
                ExportFormat::Gif => "gif",
                ExportFormat::WebM => "webm",
            };
            let destination = match unique_output(&directory, extension) {
                Ok(p) => p,
                Err(e) => {
                    ui::error(&window, &e);
                    return;
                }
            };
            let token = CancelToken::default();
            *active_cancel.borrow_mut() = Some(token.clone());
            cancel_button.set_sensitive(true);
            status.set_text("Exporting…");
            let (path, window, status, cancel_button, active_cancel, on_saved, editor_closed) = (
                path.clone(),
                window.clone(),
                status.clone(),
                cancel_button.clone(),
                active_cancel.clone(),
                on_saved.clone(),
                editor_closed.clone(),
            );
            ui::job(
                move || {
                    MediaToolchain::from_command_names()
                        .export(
                            &path,
                            &destination.path,
                            &edit,
                            &ExportSpec {
                                format,
                                quality: QualityPreset::High,
                                max_size_bytes: None,
                                frames_per_second: None,
                                gif_max_colors: Some(256),
                            },
                            &token,
                            |_| {},
                        )
                        .map_err(|e| e.to_string())?;
                    if token.is_cancelled() {
                        return Err("Export cancelled".into());
                    }
                    destination.commit()
                },
                move |result| {
                    *active_cancel.borrow_mut() = None;
                    if editor_closed.get() {
                        return;
                    }
                    cancel_button.set_sensitive(false);
                    match result {
                        Ok(saved) => {
                            status.set_text("Export complete");
                            on_saved(saved);
                        }
                        Err(error) => {
                            status.set_text("Export failed");
                            ui::error(&window, &error);
                        }
                    }
                },
            );
        });
    }
    {
        let active_cancel = active_cancel.clone();
        cancel.connect_clicked(move |_| {
            if let Some(token) = active_cancel.borrow().as_ref() {
                token.cancel();
            }
        });
    }
    {
        let (active_cancel, preview_cancel, editor_closed) = (
            active_cancel.clone(),
            preview_cancel.clone(),
            editor_closed.clone(),
        );
        window.connect_delete_event(move |_, _| {
            editor_closed.set(true);
            if let Some(token) = active_cancel.borrow().as_ref() {
                token.cancel();
            }
            if let Some(token) = preview_cancel.borrow().as_ref() {
                token.cancel();
            }
            glib::Propagation::Proceed
        });
    }
}

fn spin(min: f64, max: f64, step: f64) -> gtk::SpinButton {
    gtk::SpinButton::with_range(min, max, step)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn segment_inputs_preserve_audio_offsets_and_duration() {
        let info = RecordingSegmentInfo {
            path: "v.mp4".into(),
            system_audio_path: Some("s.wav".into()),
            system_audio_offset_ms: -13,
            system_audio_warning: None,
            microphone_path: Some("m.wav".into()),
            microphone_offset_ms: 21,
            microphone_warning: None,
            width: 640,
            height: 480,
            duration_ms: 1234,
            size_bytes: 1,
            dropped_frames: 0,
        };
        let input = segment_inputs(&[info]).remove(0);
        assert_eq!(input.system_audio_offset_ms, -13);
        assert_eq!(input.microphone_offset_ms, 21);
        assert_eq!(input.duration_ms, 1234);
    }
    #[test]
    fn unique_output_never_selects_an_existing_source() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("Capture-fixed.mp4");
        fs::write(&source, b"source").unwrap();
        let output = unique_output(dir.path(), "mp4").unwrap();
        assert!(!output.path.exists());
        fs::write(&output.path, b"new media").unwrap();
        let published = output.commit().unwrap();
        let second = output.commit().unwrap();
        assert_ne!(published, second);
        drop(output);
        assert_eq!(fs::read(published).unwrap(), b"new media");
        assert_eq!(fs::read(source).unwrap(), b"source");
    }

    #[test]
    fn named_output_uses_the_editor_filename_without_replacing_a_collision() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Demo edited.mp4"), b"existing").unwrap();
        let output = named_output(dir.path(), "Demo edited", "mp4").unwrap();
        fs::write(&output.path, b"new media").unwrap();
        let published = output.commit().unwrap();
        assert_eq!(published.file_name().unwrap(), "Demo edited 1.mp4");
        assert_eq!(
            fs::read(dir.path().join("Demo edited.mp4")).unwrap(),
            b"existing"
        );
        assert!(named_output(dir.path(), "../escape", "mp4").is_err());
    }

    #[test]
    fn private_work_directories_are_unique_and_do_not_replace_existing_paths() {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;
        let parent = tempfile::tempdir().unwrap();
        let first = create_private_work_directory(parent.path(), "recording").unwrap();
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&first).unwrap().permissions().mode() & 0o777,
            0o700
        );
        fs::write(first.join("native-segment-000.mp4"), b"first session").unwrap();
        let second = create_private_work_directory(parent.path(), "recording").unwrap();
        assert_ne!(first, second);
        assert_eq!(
            fs::read(first.join("native-segment-000.mp4")).unwrap(),
            b"first session"
        );
        assert!(!second.join("native-segment-000.mp4").exists());
    }

    #[test]
    fn recording_drafts_are_private_loadable_and_reject_escape_paths() {
        use captures_recording::{
            AudioOptions, CaptureRect, GifOptions, MaxResolution, RecordingTarget,
        };
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;
        let parent = tempfile::tempdir().unwrap();
        let options = RecordingOptions {
            kind: RecordingKind::Video,
            target: RecordingTarget::Region {
                display_id: "test".into(),
                rect: CaptureRect {
                    x: 10,
                    y: 20,
                    width: 640,
                    height: 360,
                },
            },
            frames_per_second: 30,
            max_resolution: MaxResolution::Original,
            countdown_seconds: 0,
            show_cursor: true,
            highlight_clicks: false,
            show_keystrokes: false,
            audio: AudioOptions::default(),
            gif: GifOptions::default(),
        };
        assert_eq!(
            recording_output_format(&options, ExportFormat::Gif).unwrap(),
            ExportFormat::Gif
        );
        let gif_manifest_options = recording_manifest_options(&options, ExportFormat::Gif);
        assert_eq!(gif_manifest_options.kind, RecordingKind::Gif);
        assert!(recording_output_format(&options, ExportFormat::WebM).is_err());
        let (store, manifest, directory) =
            create_recording_draft(parent.path(), &gif_manifest_options).unwrap();
        assert_eq!(manifest.options.kind, RecordingKind::Gif);
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(store.root()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(store.load(&manifest.session_id).unwrap(), manifest);
        assert!(relative_draft_path(&directory, &parent.path().join("escape.mp4")).is_err());
        fs::write(directory.join("segment.mp4"), b"media").unwrap();
        assert_eq!(
            checked_draft_media_path(&directory, "segment.mp4").unwrap(),
            directory.join("segment.mp4")
        );
        assert!(checked_draft_media_path(&directory, "../escape.mp4").is_err());
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            parent.path().join("outside.mp4"),
            directory.join("linked.mp4"),
        )
        .unwrap();
        assert!(checked_draft_media_path(&directory, "linked.mp4").is_err());
    }

    #[test]
    fn media_toolchain_applies_trim_crop_and_scale_to_generated_video() {
        let media = MediaToolchain::from_command_names();
        media
            .verify()
            .expect("Native media tests require FFmpeg and ffprobe");
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.mp4");
        let generated = std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=160x120:rate=10:duration=2",
                "-pix_fmt",
                "yuv420p",
                "-y",
            ])
            .arg(&source)
            .status()
            .unwrap();
        assert!(generated.success());
        let output = dir.path().join("edited.mp4");
        media
            .export(
                &source,
                &output,
                &EditSpec {
                    trim_start_ms: 250,
                    trim_end_ms: Some(1_250),
                    crop: Some(CropRect {
                        x: 20,
                        y: 10,
                        width: 120,
                        height: 100,
                    }),
                    output_width: Some(96),
                    output_height: Some(80),
                    audio: AudioEdit::default(),
                },
                &ExportSpec {
                    format: ExportFormat::Mp4,
                    quality: QualityPreset::High,
                    max_size_bytes: None,
                    frames_per_second: None,
                    gif_max_colors: None,
                },
                &CancelToken::default(),
                |_| {},
            )
            .unwrap();
        let probe = media.probe(&output).unwrap();
        assert_eq!((probe.metadata.width, probe.metadata.height), (96, 80));
        assert!(
            probe
                .metadata
                .duration_ms
                .is_some_and(|ms| (900..=1_100).contains(&ms))
        );

        use captures_recording::{
            AudioOptions, CaptureRect, GifOptions, MaxResolution, RecordingTarget,
        };
        let options = RecordingOptions {
            kind: RecordingKind::Video,
            target: RecordingTarget::Region {
                display_id: "test".into(),
                rect: CaptureRect {
                    x: 0,
                    y: 0,
                    width: 160,
                    height: 120,
                },
            },
            frames_per_second: 10,
            max_resolution: MaxResolution::Original,
            countdown_seconds: 0,
            show_cursor: true,
            highlight_clicks: false,
            show_keystrokes: false,
            audio: AudioOptions::default(),
            gif: GifOptions::default(),
        };
        let (store, mut manifest, draft) = create_recording_draft(dir.path(), &options).unwrap();
        fs::copy(&source, draft.join("native-segment-000.mp4")).unwrap();
        manifest.segments.push(RecordingSegmentManifest {
            index: 0,
            relative_path: "native-segment-000.mp4".into(),
            system_audio_relative_path: None,
            system_audio_offset_ms: 0,
            system_audio_warning: None,
            microphone_relative_path: None,
            microphone_offset_ms: 0,
            microphone_warning: None,
            started_at_ms: now_ms(),
            duration_ms: 2_000,
            width: 160,
            height: 120,
            size_bytes: fs::metadata(&source).unwrap().len(),
            dropped_frames: 0,
            complete: true,
        });
        manifest.state = RecordingState::Failed;
        store.save(&manifest).unwrap();
        assert_eq!(list_recording_drafts(dir.path()).unwrap().len(), 1);
        let recovered = recover_recording_draft(dir.path(), &manifest.session_id).unwrap();
        let recovered_probe = media.probe(&recovered).unwrap();
        assert_eq!(
            (
                recovered_probe.metadata.width,
                recovered_probe.metadata.height
            ),
            (160, 120)
        );
        assert!(list_recording_drafts(dir.path()).unwrap().is_empty());
    }
}
