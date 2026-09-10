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
use captures_recording::{RecordingKind, RecordingOptions, RecordingSegmentInfo};
use captures_recording_xcap::XcapRecordingSegment;
use gtk::{glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    fs,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::ui;

struct Session {
    options: RecordingOptions,
    display: DisplayDescriptor,
    output_directory: PathBuf,
    work_directory: PathBuf,
    active: Option<XcapRecordingSegment>,
    segments: Vec<RecordingSegmentInfo>,
    started: Instant,
    elapsed_before_segment: Duration,
    busy: bool,
    closing: bool,
    safety_query_in_flight: bool,
}

/// Start a real xcap/OpenH264 recording and present native recording controls.
pub fn start(
    options: RecordingOptions,
    display: DisplayDescriptor,
    directory: PathBuf,
    on_saved: Rc<dyn Fn(PathBuf)>,
) {
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("Captures recording controls");
    window.set_keep_above(true);
    window.set_decorated(false);
    window.set_resizable(false);
    window.set_type_hint(gtk::gdk::WindowTypeHint::Utility);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.set_border_width(12);
    row.style_context().add_class("glass");
    let elapsed = ui::label("Starting…", "title");
    let pause = ui::button("Pause");
    let stop = ui::button("Stop");
    stop.style_context().add_class("primary");
    let hide = ui::button("Hide");
    let discard = ui::button("Discard");
    row.pack_start(&elapsed, false, false, 0);
    row.pack_start(&pause, false, false, 0);
    row.pack_start(&stop, false, false, 0);
    row.pack_start(&hide, false, false, 0);
    row.pack_start(&discard, false, false, 0);
    window.add(&row);
    window.show_all();

    let work_directory = match create_private_work_directory(&directory, "recording") {
        Ok(path) => path,
        Err(error) => {
            ui::error(&window, &error);
            window.close();
            return;
        }
    };
    let session = Rc::new(RefCell::new(Session {
        options,
        display,
        output_directory: directory,
        work_directory,
        active: None,
        segments: Vec::new(),
        started: Instant::now(),
        elapsed_before_segment: Duration::ZERO,
        busy: false,
        closing: false,
        safety_query_in_flight: false,
    }));
    set_controls(&pause, &stop, &discard, false);
    begin_segment(&window, &elapsed, &pause, &stop, &discard, &session);

    {
        let session = session.clone();
        let elapsed = elapsed.clone();
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
            }
            glib::ControlFlow::Continue
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
        discard.connect_clicked(move |_| discard_session(&window, &session));
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
                    pause.set_label("Pause");
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
                    state.segments.push(info);
                    pause.set_label("Resume");
                    set_controls(&pause, &stop, &discard, true);
                }
                Err(error) => {
                    drop(state);
                    ui::error(&window, &error);
                    discard_session(&window, &session);
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
                    ui::error(&window_done, &error);
                    return;
                }
            };
            let (options, output_directory, work_directory, segments) = {
                let mut state = session_done.borrow_mut();
                if let Some(info) = stopped {
                    state.elapsed_before_segment += Duration::from_millis(info.duration_ms);
                    state.segments.push(info);
                }
                (
                    state.options.clone(),
                    state.output_directory.clone(),
                    state.work_directory.clone(),
                    state.segments.clone(),
                )
            };
            let (window, session, pause, stop, discard) = (
                window_done.clone(),
                session_done.clone(),
                pause_done.clone(),
                stop_done.clone(),
                discard_done.clone(),
            );
            ui::job(
                move || {
                    if segments.is_empty() {
                        return Err("The recording contains no media.".into());
                    }
                    let extension = if options.kind == RecordingKind::Gif {
                        "gif"
                    } else {
                        "mp4"
                    };
                    let destination = unique_output(&output_directory, extension)?;
                    let media = MediaToolchain::from_command_names();
                    media.verify().map_err(|e| e.to_string())?;
                    let inputs = segment_inputs(&segments);
                    let cancel = CancelToken::default();
                    if options.kind == RecordingKind::Gif {
                        let master = work_directory.join("gif-master.mp4");
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
                    cleanup_segments(&segments);
                    let _ = fs::remove_dir(&work_directory);
                    Ok(published)
                },
                move |result| match result {
                    Ok(path) => {
                        session.borrow_mut().closing = true;
                        window.close();
                        on_saved(path);
                    }
                    Err(error) => {
                        session.borrow_mut().busy = false;
                        set_controls(&pause, &stop, &discard, true);
                        ui::error(&window, &error);
                    }
                },
            );
        },
    );
}

fn discard_session(window: &gtk::Window, session: &Rc<RefCell<Session>>) {
    let (active, segments) = {
        let mut state = session.borrow_mut();
        if state.closing || state.busy {
            return;
        }
        state.closing = true;
        (state.active.take(), std::mem::take(&mut state.segments))
    };
    let work_directory = session.borrow().work_directory.clone();
    window.hide();
    let window = window.clone();
    ui::job(
        move || {
            if let Some(segment) = active {
                segment.discard().map_err(|e| e.to_string())?;
            }
            cleanup_segments(&segments);
            let _ = fs::remove_dir_all(work_directory);
            Ok(())
        },
        move |result| {
            if let Err(error) = result {
                ui::error(&window, &error);
            }
            window.close();
        },
    );
}

fn discard_segment(segment: XcapRecordingSegment) {
    std::thread::spawn(move || {
        let _ = segment.discard();
    });
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
fn cleanup_segments(segments: &[RecordingSegmentInfo]) {
    for segment in segments {
        for path in [
            Some(&segment.path),
            segment.system_audio_path.as_ref(),
            segment.microphone_path.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            let _ = fs::remove_file(path);
        }
    }
}
struct Output {
    path: PathBuf,
    work: PathBuf,
    directory: PathBuf,
    extension: String,
}
impl Output {
    fn commit(&self) -> Result<PathBuf, String> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis();
        for n in 0.. {
            let destination = self
                .directory
                .join(format!("Capture-{stamp}-{n}.{}", self.extension));
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
        match fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    unreachable!()
}

/// Open a native editor backed by `MediaToolchain` (never edits the source).
pub fn open_editor(path: PathBuf, directory: PathBuf, on_saved: Rc<dyn Fn(PathBuf)>) {
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
    fn private_work_directories_are_unique_and_do_not_replace_existing_paths() {
        use std::os::unix::fs::PermissionsExt;
        let parent = tempfile::tempdir().unwrap();
        let first = create_private_work_directory(parent.path(), "recording").unwrap();
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
    }
}
