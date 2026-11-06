//! The recording editor (`?view=recording-editor`): one History video or GIF
//! opened through the shared `RecordingEditorSession` on a worker thread.
//! The preview shows the session's retained frame as one texture, replaced
//! only when the accepted revision changes or playback delivers a frame; the
//! timeline's filmstrip is one sprite texture decoded once. Trim handles,
//! the playhead and the shade outside the trim are small nodes over it.
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use captures_app::recording_editor::{
    RecordingEditorOpenRequest, RecordingEditorRequestV2, RecordingEditorSession,
    RecordingSaveRequest, RecordingTimelineThumbnails, ReplacedRecording, SavedRecording,
};
use captures_app::recording_editor_ui::{self as ui, SaveDefaults};
use captures_app::recording_timeline::{TimelineTrimDrag, TimelineTrimEdge};
use captures_media::{
    CancelToken, CropDragHandle, CropRect, EditSpec, ExportEstimate, ExportSpec, MediaMetadata,
    MediaToolchain,
};
use captures_settings::{RecordingSettings, VideoFormat};
use gtk::prelude::*;
use gtk::{gdk, gio, glib, graphene, gsk};
use image::RgbaImage;

use crate::switch::TokenSwitch;
use crate::tokens::Tokens;
use crate::widgets::{Icon, SnapshotArea, icon_button};

mod output;
use output::{Change, Estimate, Output, Source};

pub struct Options {
    pub history_root: PathBuf,
    pub artifact_id: String,
    pub output_directory: PathBuf,
    /// Recording preferences: the preferred format and GIF defaults.
    pub preferences: RecordingSettings,
}

/// The accepted session state the UI renders.
struct Presented {
    frame: Arc<RgbaImage>,
    revision: u64,
    position_ms: u64,
    edit: EditSpec,
    save_export: ExportSpec,
    duration_ms: u64,
    mime_type: String,
    dropped_frames: u64,
    source: MediaMetadata,
    has_system_audio: bool,
    has_microphone_audio: bool,
}

impl Presented {
    fn of(session: &RecordingEditorSession) -> Self {
        let snapshot = session.snapshot_v2();
        Self {
            frame: session.frame(),
            revision: snapshot.editor.revision,
            position_ms: snapshot.editor.position_ms,
            edit: snapshot.editor.edit.clone(),
            save_export: snapshot.save_export.clone(),
            duration_ms: snapshot.editor.source.duration_ms.unwrap_or(1).max(1),
            mime_type: snapshot.editor.source.mime_type.clone(),
            dropped_frames: snapshot.editor.dropped_frames,
            source: snapshot.editor.source.clone(),
            has_system_audio: snapshot.editor.has_system_audio,
            has_microphone_audio: snapshot.editor.has_microphone_audio,
        }
    }

    fn trim_end(&self) -> u64 {
        self.edit.trim_end_ms.unwrap_or(self.duration_ms)
    }

    fn trim_end_opt(&self) -> Option<u64> {
        self.edit.trim_end_ms
    }

    fn source(&self) -> Source {
        Source {
            width: self.source.width,
            height: self.source.height,
            kind: self.source.kind,
            format: Source::format_of(&self.mime_type),
            has_system_audio: self.has_system_audio,
            has_microphone_audio: self.has_microphone_audio,
            size_bytes: self.source.size_bytes,
        }
    }
}

enum Job {
    Apply(RecordingEditorRequestV2),
    Play(u64, CancelToken),
    Save(RecordingSaveRequest, CancelToken),
    Replace(PathBuf, CancelToken),
    Thumbnails,
    /// The uncropped source frame under the crop box, for this revision.
    SourceFrame(u64),
    /// Shipping's background Est. size, for this request generation.
    Estimate(u64, CancelToken),
}

enum Outcome {
    Opened(Box<Presented>, Option<PathBuf>, SaveDefaults),
    Presented(Result<Box<Presented>, String>),
    Thumbnails(RecordingTimelineThumbnails),
    PlaybackEnded(Result<(), String>),
    Progress(String, u16),
    Saved(Result<(PathBuf, Option<Box<Presented>>), String>),
    SourceFrame(u64, Arc<RgbaImage>),
    Estimate(u64, Result<ExportEstimate, String>),
}

/// Bundled sidecars when present, else `CAPTURES_FFMPEG`/`CAPTURES_FFPROBE`,
/// else the system's `ffmpeg` and `ffprobe` on PATH.
/// The newest playback frame (position, pixels); the UI takes it at 60 Hz
/// and older frames are dropped.
type LatestFrame = Arc<Mutex<Option<(u64, Arc<RgbaImage>)>>>;

fn toolchain() -> MediaToolchain {
    let target = format!("{}-unknown-linux-gnu", std::env::consts::ARCH);
    let sidecars = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../desktop/src-tauri/binaries");
    let tool = |name: &str, variable: &str| {
        std::env::var_os(variable)
            .map(PathBuf::from)
            .or_else(|| {
                let path = sidecars.join(format!("{name}-{target}"));
                path.is_file().then_some(path)
            })
            .unwrap_or_else(|| PathBuf::from(name))
    };
    MediaToolchain::new(
        tool("ffmpeg", "CAPTURES_FFMPEG"),
        tool("ffprobe", "CAPTURES_FFPROBE"),
    )
}

fn texture_of(pixels: &RgbaImage) -> gdk::Texture {
    let (width, height) = pixels.dimensions();
    gdk::MemoryTexture::new(
        width as i32,
        height as i32,
        gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from(pixels.as_raw().as_slice()),
        width as usize * 4,
    )
    .upcast()
}

fn worker(
    options: &Options,
    jobs: mpsc::Receiver<Job>,
    out: mpsc::Sender<Outcome>,
    latest: LatestFrame,
    looping: Arc<AtomicBool>,
) {
    let (root, id, directory) = (
        options.history_root.clone(),
        options.artifact_id.clone(),
        options.output_directory.clone(),
    );
    std::thread::Builder::new()
        .name("captures-recording-editor".into())
        .spawn(move || {
            let mut session = match RecordingEditorSession::open(
                RecordingEditorOpenRequest {
                    history_root: root,
                    artifact_id: id,
                },
                toolchain(),
            ) {
                Ok(session) => {
                    let _ = out.send(Outcome::Opened(
                        Box::new(Presented::of(&session)),
                        session.original_save_path().map(Path::to_owned),
                        session.save_defaults(&directory),
                    ));
                    session
                }
                Err(error) => {
                    let _ = out.send(Outcome::Presented(Err(error)));
                    return;
                }
            };
            let progress = |progress: captures_media::ExportProgress| {
                let _ = out.send(Outcome::Progress(
                    progress
                        .message
                        .clone()
                        .unwrap_or_else(|| ui::export_stage_label(progress.stage).to_owned()),
                    progress.completed_per_mille,
                ));
            };
            while let Ok(job) = jobs.recv() {
                let outcome = match job {
                    Job::Apply(request) => Outcome::Presented(
                        session
                            .execute_v2(request)
                            .map(|()| Box::new(Presented::of(&session))),
                    ),
                    Job::Thumbnails => match session.timeline_thumbnails(&CancelToken::default()) {
                        Ok(thumbnails) => Outcome::Thumbnails(thumbnails),
                        Err(_) => continue,
                    },
                    Job::SourceFrame(revision) => {
                        match session.source_frame(&CancelToken::default()) {
                            Ok(frame) => Outcome::SourceFrame(revision, frame),
                            Err(_) => continue,
                        }
                    }
                    Job::Estimate(generation, cancel) => {
                        Outcome::Estimate(generation, session.estimate_save_export(&cancel))
                    }
                    Job::Play(position, cancel) => {
                        let result = (|| {
                            let mut playback = session.looping_playback(
                                position,
                                true,
                                looping.clone(),
                                &cancel,
                            )?;
                            while let Some(frame) = playback.next_frame()? {
                                if cancel.is_cancelled() {
                                    break;
                                }
                                *latest.lock().expect("frame") =
                                    Some((frame.position_ms, frame.pixels()));
                            }
                            Ok(())
                        })();
                        Outcome::PlaybackEnded(if cancel.is_cancelled() {
                            Ok(())
                        } else {
                            result
                        })
                    }
                    Job::Save(request, cancel) => {
                        Outcome::Saved(session.save_new(request, &cancel, progress).map(|saved| {
                            match saved {
                                SavedRecording::Saved { path, .. }
                                | SavedRecording::SavedWithoutHistory { path, .. } => (path, None),
                            }
                        }))
                    }
                    Job::Replace(destination, cancel) => Outcome::Saved(
                        session
                            .replace_original_at(&destination, &cancel, progress)
                            .map(|ReplacedRecording::Replaced { path, .. }| {
                                (path, Some(Box::new(Presented::of(&session))))
                            })
                            .map_err(|error| error.message),
                    ),
                };
                if out.send(outcome).is_err() {
                    break;
                }
            }
        })
        .expect("recording editor worker");
}

pub struct RecordingEditor {
    pub window: gtk::ApplicationWindow,
    _state: Rc<State>,
}

struct State {
    tokens: Rc<Tokens>,
    jobs: mpsc::Sender<Job>,
    latest: LatestFrame,
    looping: Arc<AtomicBool>,
    presented: RefCell<Option<Presented>>,
    frame: RefCell<Option<gdk::Texture>>,
    filmstrip: RefCell<Option<gdk::Texture>>,
    /// Staged trim and playhead (source-relative ms) while the session is
    /// behind; applied after `LIVE_APPLY_DELAY_MS` of quiet.
    trim: Cell<(f64, f64)>,
    playhead: Cell<u64>,
    drag: Cell<Option<(TimelineTrimEdge, TimelineTrimDrag)>>,
    pending_apply: RefCell<Option<glib::SourceId>>,
    playing: RefCell<Option<CancelToken>>,
    one_to_one: Cell<bool>,
    original: RefCell<Option<PathBuf>>,
    /// The default save name, before Save as new adds `-edited`.
    source_stem: RefCell<String>,
    directory: RefCell<PathBuf>,
    busy: Cell<bool>,
    /// An export is running; playback waits for it.
    saving: Cell<bool>,
    preferences: RecordingSettings,
    output: Rc<Output>,
    /// The uncropped source frame (and its revision) under the crop box.
    source_frame: RefCell<Option<(u64, gdk::Texture)>>,
    /// Where the preview last drew its frame, in widget points.
    image_bounds: Cell<(f32, f32, f32, f32)>,
    crop_drag: Cell<Option<(CropDragHandle, CropRect)>>,
    estimate: Cell<Estimate>,
    /// Supersedes the running estimate and names the next one.
    estimate_generation: Cell<u64>,
    estimate_cancel: RefCell<Option<CancelToken>>,
    estimate_timer: RefCell<Option<glib::SourceId>>,
    /// The running export's token, for Cancel.
    export_cancel: RefCell<Option<CancelToken>>,
    /// The last saved file and the settings it was saved with: Save stays
    /// off until something changes (`alreadySaved`).
    saved: RefCell<Option<(PathBuf, String)>>,
    /// Clears a success toast after shipping's 4 s.
    toast_timer: RefCell<Option<glib::SourceId>>,
    #[allow(deprecated)] // `GtkFileDialog` needs GTK 4.10.
    chooser: RefCell<Option<gtk::FileChooserNative>>,
    w: Widgets,
}

struct Widgets {
    title: gtk::Label,
    warning: gtk::Label,
    preview: SnapshotArea,
    play: gtk::Button,
    play_icon: Icon,
    loop_button: gtk::Button,
    fit: gtk::Button,
    actual: gtk::Button,
    range: gtk::Label,
    selected: gtk::Label,
    track: SnapshotArea,
    track_overlay: gtk::Overlay,
    handles: [gtk::Scale; 2],
    folder: gtk::Label,
    change_folder: gtk::Button,
    filename: gtk::Entry,
    save_as_new: TokenSwitch,
    save: gtk::Button,
    cancel: gtk::Button,
    show_in_folder: gtk::Button,
    progress: gtk::ProgressBar,
    status: gtk::Label,
}

fn label(text: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class(class);
    label.set_xalign(0.);
    label
}

fn segment(text: &str) -> gtk::Button {
    let button = gtk::Button::with_label(text);
    button.add_css_class("recording-segment");
    button
}

fn set_class(widget: &impl IsA<gtk::Widget>, class: &str, on: bool) {
    if on {
        widget.add_css_class(class);
    } else {
        widget.remove_css_class(class);
    }
}

/// `.timeline-trim-handle`: twice the 4 px track inset, 3 px past each edge.
const HANDLE_WIDTH: i32 = 8;
const HANDLE_OVERHANG: i32 = 3;

pub fn build(app: &gtk::Application, tokens: Rc<Tokens>, options: Options) -> RecordingEditor {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(captures_app::app_windows::RECORDING_EDITOR_TITLE)
        .default_width(1100)
        .default_height(820)
        .build();
    window.add_css_class("captures");
    window.set_size_request(760, 560);

    let (jobs, inbox) = mpsc::channel();
    let (results, outcomes) = mpsc::channel();
    let latest = LatestFrame::default();
    let looping = Arc::<AtomicBool>::default();
    worker(&options, inbox, results, latest.clone(), looping.clone());

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("recording-editor");
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .build();
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("recording-editor-body");
    let title = label(ui::TITLE_RECORDING, "recording-editor-title");
    title.set_accessible_role(gtk::AccessibleRole::Heading);
    body.append(&title);
    let warning = label("", "recording-editor-warning");
    warning.set_wrap(true);
    warning.set_visible(false);
    body.append(&warning);

    // `.recording-editor-preview`: toolbar over the sunken viewport.
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("recording-card");
    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    toolbar.add_css_class("recording-preview-toolbar");
    let heading = label("Preview", "recording-preview-heading");
    heading.set_hexpand(true);
    toolbar.append(&heading);
    let (loop_button, _, _) = icon_button("loop", "Loop preview", &["recording-loop"]);
    toolbar.append(&loop_button);
    let zoom = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    zoom.add_css_class("recording-segments");
    zoom.set_valign(gtk::Align::Center);
    let fit = segment("Fit");
    let actual = segment("100%");
    zoom.append(&fit);
    zoom.append(&actual);
    toolbar.append(&zoom);
    card.append(&toolbar);
    let viewport = gtk::Overlay::new();
    viewport.add_css_class("recording-preview-viewport");
    let preview = SnapshotArea::new(|_, _, _| {});
    preview.set_size_request(-1, 380);
    preview.set_hexpand(true);
    preview.set_accessible_role(gtk::AccessibleRole::Img);
    preview.update_property(&[gtk::accessible::Property::Label("Recording preview")]);
    viewport.set_child(Some(&preview));
    let play = gtk::Button::new();
    play.add_css_class("recording-play");
    let play_icon = Icon::new("resume", 22).with_stroke(2.);
    play.set_child(Some(&play_icon));
    play.set_halign(gtk::Align::Center);
    play.set_valign(gtk::Align::Center);
    play.update_property(&[gtk::accessible::Property::Label("Play preview")]);
    viewport.add_overlay(&play);
    card.append(&viewport);
    body.append(&card);

    // `.timeline`: summary row over the filmstrip track.
    let timeline = gtk::Box::new(gtk::Orientation::Vertical, 0);
    timeline.add_css_class("recording-card");
    timeline.add_css_class("recording-timeline");
    let summary = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    summary.add_css_class("timeline-summary");
    let range = label("", "timeline-range");
    range.set_hexpand(true);
    let selected = label("", "timeline-selected");
    summary.append(&range);
    summary.append(&selected);
    timeline.append(&summary);
    let track_overlay = gtk::Overlay::new();
    let track = SnapshotArea::new(|_, _, _| {});
    track.add_css_class("timeline-track");
    track.set_size_request(-1, ui::TIMELINE_TRACK_HEIGHT as i32);
    track.set_hexpand(true);
    track.set_cursor_from_name(Some("col-resize"));
    track.set_margin_top(HANDLE_OVERHANG);
    track.set_margin_bottom(HANDLE_OVERHANG);
    track.update_property(&[gtk::accessible::Property::Label("Timeline")]);
    track_overlay.set_child(Some(&track));
    let handles = ["Trim start", "Trim end"].map(|name| {
        // A restyled GtkRange: real slider semantics (AT-SPI Value on every
        // GTK 4 release) drawn as `.timeline-trim-handle`'s accent bar.
        let handle = gtk::Scale::new(
            gtk::Orientation::Horizontal,
            Some(&gtk::Adjustment::new(0., 0., 1., 100., 1_000., 0.)),
        );
        handle.add_css_class("timeline-handle");
        handle.set_draw_value(false);
        handle.set_halign(gtk::Align::Start);
        handle.set_size_request(
            HANDLE_WIDTH,
            ui::TIMELINE_TRACK_HEIGHT as i32 + 2 * HANDLE_OVERHANG,
        );
        handle.set_focusable(true);
        handle.set_can_target(false);
        handle.update_property(&[gtk::accessible::Property::Label(name)]);
        track_overlay.add_overlay(&handle);
        handle
    });
    timeline.append(&track_overlay);
    body.append(&timeline);
    scroller.set_child(Some(&body));
    root.append(&scroller);

    // The save footer (`.recording-save-bar`).
    // `.recording-save-footer` centers its filename, switch and action columns.
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    bar.add_css_class("recording-save-bar");
    let columns = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    columns.set_halign(gtk::Align::Center);
    columns.set_hexpand(true);
    bar.append(&columns);
    let file = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let file_heading = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    file_heading.append(&label("Filename", "editor-field-label"));
    let folder = label("", "editor-folder");
    folder.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    folder.set_max_width_chars(36);
    file_heading.append(&folder);
    // Shipping `aria-label="Change save location"` over "Change…"; the text
    // is presentational because GTK 4.14 names a button after its label.
    let change_folder = gtk::Button::new();
    let change_text = gtk::Label::new(Some("Change…"));
    change_text.set_accessible_role(gtk::AccessibleRole::Presentation);
    change_folder.set_child(Some(&change_text));
    change_folder.add_css_class("editor-folder-change");
    change_folder.update_property(&[gtk::accessible::Property::Label("Change save location")]);
    file_heading.append(&change_folder);
    file.append(&file_heading);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    row.add_css_class("editor-filename");
    let filename = gtk::Entry::new();
    filename.add_css_class("editor-filename-entry");
    filename.set_width_chars(30);
    filename.update_property(&[gtk::accessible::Property::Label("Saved filename")]);
    let state_cell: Rc<RefCell<std::rc::Weak<State>>> = Rc::default();
    let output = Output::new({
        let state_cell = state_cell.clone();
        move |change| {
            if let Some(state) = state_cell.borrow().upgrade() {
                state.output_changed(change);
            }
        }
    });
    body.append(&output.root);
    row.append(&filename);
    row.append(&output.format.root);
    file.append(&row);
    columns.append(&file);
    let as_new = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    as_new.set_valign(gtk::Align::End);
    let save_as_new = TokenSwitch::new(tokens.clone(), false);
    save_as_new.update_property(&[gtk::accessible::Property::Label("Save as new file")]);
    as_new.append(&save_as_new);
    as_new.append(&label("Save as new file", "editor-switch-label"));
    columns.append(&as_new);
    // `.recording-save-action-area`: the one-line toast over the actions.
    let area = gtk::Box::new(gtk::Orientation::Vertical, 3);
    area.set_size_request(340, -1);
    area.set_valign(gtk::Align::End);
    let status = label("", "recording-save-toast");
    status.set_xalign(1.);
    status.set_ellipsize(gtk::pango::EllipsizeMode::End);
    status.set_size_request(-1, 16);
    status.set_accessible_role(gtk::AccessibleRole::Status);
    area.append(&status);
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    actions.set_halign(gtk::Align::End);
    actions.add_css_class("recording-save-actions");
    let cancel = gtk::Button::with_label("Cancel");
    cancel.add_css_class("ui-btn");
    cancel.add_css_class("recording-save-cancel");
    cancel.set_size_request(76, -1);
    actions.append(&cancel);
    let (show_in_folder, _, _) = icon_button(
        "folder",
        "Show in Folder",
        &["ui-btn", "recording-show-in-folder"],
    );
    show_in_folder.set_size_request(140, -1);
    actions.append(&show_in_folder);
    let (save, _, _) = icon_button("save", "Save", &["ui-btn", "primary", "editor-save"]);
    save.set_size_request(100, -1);
    save.set_sensitive(false);
    actions.append(&save);
    area.append(&actions);
    columns.append(&area);
    // `.recording-export-progress`: a 3 px strip along the footer's top.
    let footer = gtk::Overlay::new();
    footer.set_child(Some(&bar));
    let progress = gtk::ProgressBar::new();
    progress.add_css_class("recording-export-progress");
    progress.set_valign(gtk::Align::Start);
    progress.set_visible(false);
    progress.set_can_target(false);
    footer.add_overlay(&progress);
    root.append(&footer);
    window.set_child(Some(&root));

    let state = Rc::new(State {
        tokens,
        jobs,
        latest,
        looping,
        presented: RefCell::default(),
        frame: RefCell::default(),
        filmstrip: RefCell::default(),
        trim: Cell::new((0., 1.)),
        playhead: Cell::new(0),
        drag: Cell::new(None),
        pending_apply: RefCell::default(),
        playing: RefCell::default(),
        one_to_one: Cell::new(false),
        original: RefCell::default(),
        source_stem: RefCell::default(),
        directory: RefCell::new(options.output_directory),
        busy: Cell::new(true),
        saving: Cell::default(),
        preferences: options.preferences,
        output,
        source_frame: RefCell::default(),
        image_bounds: Cell::default(),
        crop_drag: Cell::default(),
        estimate: Cell::default(),
        estimate_generation: Cell::default(),
        estimate_cancel: RefCell::default(),
        estimate_timer: RefCell::default(),
        chooser: RefCell::default(),
        export_cancel: RefCell::default(),
        saved: RefCell::default(),
        toast_timer: RefCell::default(),
        w: Widgets {
            title,
            warning,
            preview,
            play,
            play_icon,
            loop_button,
            fit,
            actual,
            range,
            selected,
            track,
            track_overlay,
            handles,
            folder,
            change_folder,
            filename,
            save_as_new,
            save,
            cancel,
            show_in_folder,
            progress,
            status,
        },
    });
    *state_cell.borrow_mut() = Rc::downgrade(&state);
    state.connect(&window);
    state.set_zoom(false);
    // Start on Play, not the filename entry, so nothing looks edited.
    GtkWindowExt::set_focus(&window, Some(&state.w.play));

    let weak = Rc::downgrade(&state);
    glib::timeout_add_local(Duration::from_millis(16), move || {
        let Some(state) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        while let Ok(outcome) = outcomes.try_recv() {
            state.receive(outcome);
        }
        state.present_playback_frame();
        glib::ControlFlow::Continue
    });
    let weak = Rc::downgrade(&state);
    window.connect_close_request(move |_| {
        if let Some(state) = weak.upgrade()
            && let Some(cancel) = state.playing.borrow_mut().take()
        {
            cancel.cancel();
        }
        glib::Propagation::Proceed
    });
    RecordingEditor {
        window,
        _state: state,
    }
}

impl RecordingEditor {
    pub fn present(&self) {
        self.window.present();
    }

    /// `--exercise`: move the trim start in or out by a second and re-apply.
    pub fn nudge_trim(&self, inward: bool) {
        let state = &self._state;
        let (start, end) = state.trim.get();
        let start = if inward {
            (start + 1_000.).min(end - 100.)
        } else {
            0.
        };
        state.trim.set((start.max(0.), end));
        state.trim_changed();
        state.schedule_apply();
    }
}

impl State {
    fn connect(self: &Rc<Self>, window: &gtk::ApplicationWindow) {
        let w = &self.w;
        let weak = Rc::downgrade(self);
        w.preview.set_draw(move |snapshot, width, height| {
            if let Some(state) = weak.upgrade() {
                state.draw_preview(snapshot, width, height);
            }
        });
        let weak = Rc::downgrade(self);
        w.track.set_draw(move |snapshot, width, height| {
            if let Some(state) = weak.upgrade() {
                state.draw_track(snapshot, width, height);
            }
        });
        let weak = Rc::downgrade(self);
        w.play.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                state.toggle_play();
            }
        });
        let weak = Rc::downgrade(self);
        w.loop_button.connect_clicked(move |button| {
            if let Some(state) = weak.upgrade() {
                let on = !state.looping.load(std::sync::atomic::Ordering::Relaxed);
                state
                    .looping
                    .store(on, std::sync::atomic::Ordering::Relaxed);
                set_class(button, "active", on);
                button.update_state(&[gtk::accessible::State::Pressed(if on {
                    gtk::AccessibleTristate::True
                } else {
                    gtk::AccessibleTristate::False
                })]);
            }
        });
        for (button, one_to_one) in [(&w.fit, false), (&w.actual, true)] {
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(state) = weak.upgrade() {
                    state.set_zoom(one_to_one);
                }
            });
        }
        let weak = Rc::downgrade(self);
        w.save.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                state.save();
            }
        });
        let weak = Rc::downgrade(self);
        w.cancel.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade()
                && let Some(cancel) = state.export_cancel.borrow().as_ref()
            {
                cancel.cancel();
            }
        });
        let weak = Rc::downgrade(self);
        w.show_in_folder.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade()
                && let Some((path, _)) = state.saved.borrow().as_ref()
            {
                crate::history::reveal(path);
            }
        });
        let weak = Rc::downgrade(self);
        w.change_folder.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                state.choose_folder();
            }
        });
        let weak = Rc::downgrade(self);
        w.filename.connect_changed(move |_| {
            if let Some(state) = weak.upgrade() {
                state.update_footer();
            }
        });
        let weak = Rc::downgrade(self);
        // `setMakeCopy`: a copy beside the original gets the `-edited` name,
        // and turning it off restores the source name if it was untouched.
        w.save_as_new.connect_toggled(move |on| {
            let Some(state) = weak.upgrade() else { return };
            let source = state.source_stem.borrow().clone();
            let edited = ui::edited_file_stem(&source);
            let current = state.w.filename.text();
            if on && current == source {
                state.w.filename.set_text(&edited);
            } else if !on && current == edited {
                state.w.filename.set_text(&source);
            }
            state.update_footer();
        });

        // Preview: drag the crop box or a handle in source pixels.
        let crop_drag = gtk::GestureDrag::new();
        let weak = Rc::downgrade(self);
        crop_drag.connect_drag_begin(move |gesture, x, y| {
            let Some(state) = weak.upgrade() else { return };
            match (state.crop_handle_at(x, y), state.output.crop_shown()) {
                (Some(handle), Some(crop)) => state.crop_drag.set(Some((handle, crop))),
                _ => {
                    gesture.set_state(gtk::EventSequenceState::Denied);
                }
            }
        });
        let weak = Rc::downgrade(self);
        crop_drag.connect_drag_update(move |_, dx, dy| {
            let Some(state) = weak.upgrade() else { return };
            let (Some((handle, from)), Some((_, texture))) =
                (state.crop_drag.get(), state.crop_view())
            else {
                return;
            };
            let scale = f64::from(state.image_bounds.get().2) / f64::from(texture.width().max(1));
            state
                .output
                .drag_crop(from, handle, dx / scale.max(1e-6), dy / scale.max(1e-6));
            state.w.preview.queue_draw();
        });
        let weak = Rc::downgrade(self);
        crop_drag.connect_drag_end(move |_, _, _| {
            if let Some(state) = weak.upgrade()
                && state.crop_drag.take().is_some()
            {
                state.output_changed(Change::Edit);
            }
        });
        w.preview.add_controller(crop_drag);
        let motion = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(self);
        motion.connect_motion(move |_, x, y| {
            let Some(state) = weak.upgrade() else { return };
            use CropDragHandle::*;
            let cursor = state.crop_handle_at(x, y).map(|handle| match handle {
                Move => "move",
                North | South => "ns-resize",
                East | West => "ew-resize",
                NorthEast | SouthWest => "nesw-resize",
                NorthWest | SouthEast => "nwse-resize",
            });
            state.w.preview.set_cursor_from_name(cursor);
        });
        w.preview.add_controller(motion);

        // Track: drag a handle to trim, or press elsewhere to seek.
        let drag = gtk::GestureDrag::new();
        let weak = Rc::downgrade(self);
        drag.connect_drag_begin(move |_, x, _| {
            if let Some(state) = weak.upgrade() {
                state.track_begin(x);
            }
        });
        let weak = Rc::downgrade(self);
        drag.connect_drag_update(move |gesture, dx, _| {
            if let (Some(state), Some((x, _))) = (weak.upgrade(), gesture.start_point()) {
                state.track_update(x + dx);
            }
        });
        let weak = Rc::downgrade(self);
        drag.connect_drag_end(move |_, _, _| {
            if let Some(state) = weak.upgrade()
                && state.drag.take().is_some()
            {
                state.schedule_apply();
            }
        });
        w.track.add_controller(drag);
        let weak = Rc::downgrade(self);
        // Handles follow the trim at every allocation, so resizing keeps them
        // on the selection without a resize signal.
        w.track_overlay.connect_get_child_position(move |_, child| {
            let state = weak.upgrade()?;
            let index = state.w.handles.iter().position(|handle| handle == child)?;
            let (left, width) = state.track_rect();
            let value = [state.trim.get().0, state.trim.get().1][index];
            let x = left + value / state.duration().max(1) as f64 * width;
            Some(gdk::Rectangle::new(
                x as i32 - HANDLE_WIDTH / 2,
                0,
                HANDLE_WIDTH,
                ui::TIMELINE_TRACK_HEIGHT as i32 + 2 * HANDLE_OVERHANG,
            ))
        });

        // Keyboard trim: arrows move the focused handle by 100 ms (1 s with Shift).
        for (index, handle) in w.handles.iter().enumerate() {
            let keys = gtk::EventControllerKey::new();
            keys.set_propagation_phase(gtk::PropagationPhase::Capture);
            let weak = Rc::downgrade(self);
            keys.connect_key_pressed(move |_, key, _, modifiers| {
                let Some(state) = weak.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                let step = if modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
                    1_000.
                } else {
                    100.
                };
                let delta = match key {
                    gdk::Key::Left => -step,
                    gdk::Key::Right => step,
                    _ => return glib::Propagation::Proceed,
                };
                let (start, end) = state.trim.get();
                let duration = state.duration() as f64;
                state.trim.set(if index == 0 {
                    (((start + delta).clamp(0., end - 1.)), end)
                } else {
                    (start, (end + delta).clamp(start + 1., duration))
                });
                state.trim_changed();
                state.schedule_apply();
                glib::Propagation::Stop
            });
            handle.add_controller(keys);
            // Assistive technologies set the value directly.
            let weak = Rc::downgrade(self);
            handle.connect_value_changed(move |handle| {
                let Some(state) = weak.upgrade() else { return };
                let (start, end) = state.trim.get();
                let value = handle.value();
                let next = if index == 0 {
                    (value.clamp(0., end - 1.), end)
                } else {
                    (start, value.clamp(start + 1., state.duration() as f64))
                };
                if next != (start, end) {
                    state.trim.set(next);
                    state.trim_changed();
                    state.schedule_apply();
                }
            });
        }

        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(state) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let typing = state.w.filename.has_focus()
                || state
                    .w
                    .preview
                    .root()
                    .and_then(|root| root.focus())
                    .is_some_and(|focus| focus.is::<gtk::Text>());
            if key == gdk::Key::space && !typing {
                state.toggle_play();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        window.add_controller(keys);
    }

    fn duration(&self) -> u64 {
        self.presented
            .borrow()
            .as_ref()
            .map_or(1, |presented| presented.duration_ms)
    }

    fn receive(self: &Rc<Self>, outcome: Outcome) {
        match outcome {
            Outcome::Opened(presented, original, defaults) => {
                let gif_source = presented.mime_type.contains("gif");
                self.w.title.set_label(ui::title(&presented.mime_type));
                if let Some(text) = ui::dropped_frames_warning(presented.dropped_frames) {
                    self.w.warning.set_label(&text);
                    self.w.warning.set_visible(true);
                }
                *self.original.borrow_mut() = original;
                *self.directory.borrow_mut() = PathBuf::from(&defaults.directory);
                *self.source_stem.borrow_mut() = defaults.stem.clone();
                self.w.filename.set_text(&defaults.stem);
                self.w.loop_button.set_visible(!gif_source);
                self.output.open(presented.source(), &self.preferences);
                let initial = self
                    .output
                    .specs(presented.edit.trim_start_ms, presented.edit.trim_end_ms);
                let accepted = (presented.edit.clone(), presented.save_export.clone());
                self.accept(*presented);
                let _ = self.jobs.send(Job::Thumbnails);
                // Shipping's initial format, quality and GIF defaults.
                if initial.is_some_and(|initial| initial != accepted) {
                    self.output_changed(Change::Format);
                }
            }
            Outcome::Presented(Ok(presented)) => self.accept(*presented),
            Outcome::Presented(Err(error)) => {
                self.busy.set(false);
                self.notify(&error, true);
            }
            Outcome::SourceFrame(revision, frame) => {
                *self.source_frame.borrow_mut() = Some((revision, texture_of(&frame)));
                self.w.preview.queue_draw();
            }
            Outcome::Estimate(generation, result) => {
                if generation == self.estimate_generation.get() {
                    self.estimate_cancel.borrow_mut().take();
                    // Shipping never shows an estimate failure as an error.
                    self.estimate.set(match result {
                        Ok(estimate) => Estimate {
                            bytes: Some(estimate.size_bytes),
                            exact: estimate.exact,
                            pending: false,
                        },
                        Err(_) => Estimate::default(),
                    });
                    self.output.sync(self.estimate.get());
                }
            }
            Outcome::Thumbnails(thumbnails) => {
                *self.filmstrip.borrow_mut() = Some(texture_of(thumbnails.pixels()));
                self.w.track.queue_draw();
            }
            Outcome::PlaybackEnded(result) => {
                self.playing.borrow_mut().take();
                self.set_playing_ui(false);
                if let Err(error) = result {
                    self.notify(&error, true);
                }
                // Retain the paused frame as the accepted position.
                let _ = self.jobs.send(Job::Apply(RecordingEditorRequestV2::Seek {
                    position_ms: self.playhead.get(),
                }));
            }
            Outcome::Progress(text, per_mille) => {
                self.notify(&text, false);
                self.w.progress.set_visible(true);
                self.w
                    .progress
                    .set_fraction(f64::from(per_mille.min(1_000)) / 1_000.);
            }
            Outcome::Saved(result) => {
                self.busy.set(false);
                self.saving.set(false);
                self.w.progress.set_visible(false);
                let cancelled = self
                    .export_cancel
                    .borrow_mut()
                    .take()
                    .is_some_and(|cancel| cancel.is_cancelled());
                match result {
                    Ok((path, presented)) => {
                        let gif = self.output.format() == VideoFormat::Gif;
                        let size = std::fs::metadata(&path).map_or(0, |m| m.len());
                        if let Some(presented) = presented {
                            *self.original.borrow_mut() = Some(path.clone());
                            self.accept(*presented);
                        }
                        *self.saved.borrow_mut() = Some((path, self.fingerprint()));
                        self.toast(&ui::saved_message(gif, size));
                    }
                    Err(_) if cancelled => self.toast("Save cancelled."),
                    Err(error) => self.notify(&error, true),
                }
                self.update_footer();
            }
        }
    }

    /// The session accepted an edit or seek: refresh the retained frame only
    /// when the revision moved.
    fn accept(self: &Rc<Self>, presented: Presented) {
        let changed = self
            .presented
            .borrow()
            .as_ref()
            .is_none_or(|old| old.revision != presented.revision);
        if changed && self.playing.borrow().is_none() {
            *self.frame.borrow_mut() = Some(texture_of(&presented.frame));
            self.w.preview.queue_draw();
        }
        let revision = presented.revision;
        if self.drag.get().is_none() && self.pending_apply.borrow().is_none() {
            self.trim.set((
                presented.edit.trim_start_ms as f64,
                presented.trim_end() as f64,
            ));
        }
        if self.playing.borrow().is_none() {
            self.playhead.set(presented.position_ms);
        }
        *self.presented.borrow_mut() = Some(presented);
        self.busy.set(false);
        self.trim_changed();
        self.update_footer();
        if changed {
            self.request_source_frame(revision);
            self.schedule_estimate();
        }
        self.output.sync(self.estimate.get());
    }

    /// The crop box needs the uncropped frame at the accepted position.
    fn request_source_frame(&self, revision: u64) {
        let current = self
            .source_frame
            .borrow()
            .as_ref()
            .is_some_and(|(shown, _)| *shown == revision);
        if self.output.crop_shown().is_some() && self.playing.borrow().is_none() && !current {
            let _ = self.jobs.send(Job::SourceFrame(revision));
        }
    }

    /// A card control changed: show it, then apply it live.
    fn output_changed(self: &Rc<Self>, change: Change) {
        if change == Change::Format {
            // Shipping `updateOutputFormat`: another format saves a copy.
            let source = self.output.source().map(|source| source.format);
            if source != Some(self.output.format()) && !self.w.save_as_new.is_on() {
                self.w.save_as_new.activate_toggle();
            }
        }
        if change == Change::CropShown
            && let Some(revision) = self.presented.borrow().as_ref().map(|p| p.revision)
        {
            self.request_source_frame(revision);
        }
        self.output.sync(self.estimate.get());
        self.w.preview.queue_draw();
        self.schedule_apply();
    }

    /// Supersede any estimate; a new one starts after shipping's debounce.
    fn cancel_estimate(&self) {
        if let Some(id) = self.estimate_timer.borrow_mut().take() {
            id.remove();
        }
        if let Some(cancel) = self.estimate_cancel.borrow_mut().take() {
            cancel.cancel();
        }
        self.estimate_generation
            .set(self.estimate_generation.get() + 1);
        let mut estimate = self.estimate.get();
        estimate.pending = false;
        self.estimate.set(estimate);
    }

    fn schedule_estimate(self: &Rc<Self>) {
        self.cancel_estimate();
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(
            Duration::from_millis(ui::ESTIMATE_DEBOUNCE_MS),
            move || {
                let Some(state) = weak.upgrade() else { return };
                state.estimate_timer.borrow_mut().take();
                if !state.output.wants_estimate() || state.saving.get() {
                    return;
                }
                let cancel = CancelToken::default();
                *state.estimate_cancel.borrow_mut() = Some(cancel.clone());
                let mut estimate = state.estimate.get();
                estimate.pending = true;
                state.estimate.set(estimate);
                state.output.sync(estimate);
                let _ = state
                    .jobs
                    .send(Job::Estimate(state.estimate_generation.get(), cancel));
            },
        );
        *self.estimate_timer.borrow_mut() = Some(id);
    }

    fn present_playback_frame(&self) {
        if self.playing.borrow().is_none() {
            return;
        }
        let Some((position, pixels)) = self.latest.lock().expect("frame").take() else {
            return;
        };
        self.playhead.set(position);
        *self.frame.borrow_mut() = Some(texture_of(&pixels));
        self.w.preview.queue_draw();
        self.w.track.queue_draw();
    }

    fn toggle_play(&self) {
        if let Some(cancel) = self.playing.borrow_mut().take() {
            cancel.cancel();
            self.set_playing_ui(false);
            return;
        }
        // A pending trim apply is fine: the worker plays after it. Saving is not.
        if self.presented.borrow().is_none() || self.saving.get() {
            return;
        }
        let (start, end) = self.trim.get();
        let mut from = self.playhead.get();
        if (from as f64) < start || (from as f64) >= end - 1. {
            from = start as u64;
        }
        let cancel = CancelToken::default();
        *self.playing.borrow_mut() = Some(cancel.clone());
        self.set_playing_ui(true);
        let _ = self.jobs.send(Job::Play(from, cancel));
    }

    fn set_playing_ui(&self, playing: bool) {
        self.w
            .play_icon
            .set_name(if playing { "pause" } else { "resume" });
        set_class(&self.w.play, "playing", playing);
        let name = if playing {
            "Pause preview"
        } else {
            "Play preview"
        };
        self.w.play.set_tooltip_text(Some(name));
        self.w
            .play
            .update_property(&[gtk::accessible::Property::Label(name)]);
    }

    fn set_zoom(&self, one_to_one: bool) {
        self.one_to_one.set(one_to_one);
        set_class(&self.w.fit, "active", !one_to_one);
        set_class(&self.w.actual, "active", one_to_one);
        self.w.preview.queue_draw();
    }

    fn track_rect(&self) -> (f64, f64) {
        let inset = 4.;
        (
            inset,
            (f64::from(self.w.track.width()) - 2. * inset).max(1.),
        )
    }

    fn track_begin(&self, x: f64) {
        let duration = self.duration() as f64;
        let (left, width) = self.track_rect();
        let (start, end) = self.trim.get();
        let to_x = |time: f64| left + time / duration * width;
        let near = |time: f64| (x - to_x(time)).abs() <= f64::from(HANDLE_WIDTH);
        let edge = if near(start) && (!near(end) || (x - to_x(start)) < (to_x(end) - x)) {
            Some(TimelineTrimEdge::Start)
        } else if near(end) {
            Some(TimelineTrimEdge::End)
        } else {
            None
        };
        match edge.and_then(|edge| {
            TimelineTrimDrag::begin(edge, x, start, end, duration).map(|drag| (edge, drag))
        }) {
            Some(drag) => {
                if let Some(cancel) = self.playing.borrow_mut().take() {
                    cancel.cancel();
                }
                self.drag.set(Some(drag));
            }
            None => self.seek_to_x(x),
        }
    }

    fn track_update(&self, x: f64) {
        let Some((edge, drag)) = self.drag.get() else {
            self.seek_to_x(x);
            return;
        };
        let (left, width) = self.track_rect();
        let Some(update) = drag.update(x, left, width) else {
            return;
        };
        self.drag.set(Some((edge, update.drag)));
        let (start, end) = self.trim.get();
        self.trim.set(match edge {
            TimelineTrimEdge::Start => (update.time_ms, end),
            TimelineTrimEdge::End => (start, update.time_ms),
        });
        self.trim_changed();
    }

    fn seek_to_x(&self, x: f64) {
        let (left, width) = self.track_rect();
        let Some(time) = captures_app::recording_timeline::timeline_time_at_client_x(
            x,
            left,
            width,
            self.duration() as f64,
        ) else {
            return;
        };
        if let Some(cancel) = self.playing.borrow_mut().take() {
            cancel.cancel();
        }
        self.playhead.set(time as u64);
        self.w.track.queue_draw();
        let _ = self.jobs.send(Job::Apply(RecordingEditorRequestV2::Seek {
            position_ms: time as u64,
        }));
    }

    /// Redraw the trim nodes and labels for the staged trim.
    fn trim_changed(&self) {
        let (start, end) = self.trim.get();
        let duration = self.duration();
        let summary = ui::trim_summary(start as u64, end as u64, duration);
        self.w.range.set_label(&summary.range);
        self.w.selected.set_label(&summary.selected);
        for (handle, value) in self.w.handles.iter().zip([start, end]) {
            let adjustment = handle.adjustment();
            adjustment.set_upper(duration as f64);
            adjustment.set_value(value);
            handle.update_property(&[gtk::accessible::Property::ValueText(
                &ui::format_editor_time(value as u64, duration),
            )]);
        }
        self.place_handles();
        self.w.track.queue_draw();
    }

    fn place_handles(&self) {
        self.w.track_overlay.queue_allocate();
    }

    fn schedule_apply(self: &Rc<Self>) {
        if let Some(id) = self.pending_apply.borrow_mut().take() {
            id.remove();
        }
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(
            Duration::from_millis(ui::LIVE_APPLY_DELAY_MS),
            move || {
                let Some(state) = weak.upgrade() else { return };
                state.pending_apply.borrow_mut().take();
                let (start, end) = state.trim.get();
                let duration = state.duration();
                let end = (end.round() as u64).min(duration);
                // An invalid Maximum limit stays unapplied, and Save waits.
                let Some((edit, export)) = state
                    .output
                    .specs(start.round() as u64, (end < duration).then_some(end))
                else {
                    state.update_footer();
                    return;
                };
                state.cancel_estimate();
                state.busy.set(true);
                state.update_footer();
                let _ = state
                    .jobs
                    .send(Job::Apply(RecordingEditorRequestV2::UpdatePreview {
                        edit,
                        export,
                    }));
            },
        );
        *self.pending_apply.borrow_mut() = Some(id);
    }

    /// Everything a save depends on (`recordingEditorFingerprint`).
    fn fingerprint(&self) -> String {
        let (start, end) = self.trim.get();
        format!(
            "{:?}|{}|{}",
            self.output
                .specs(start.round() as u64, Some(end.round() as u64)),
            self.destination().display(),
            self.saving_copy(),
        )
    }

    /// A success toast, cleared after shipping's 4 s.
    fn toast(self: &Rc<Self>, text: &str) {
        self.notify(text, false);
        set_class(&self.w.status, "success", true);
        if let Some(id) = self.toast_timer.borrow_mut().take() {
            id.remove();
        }
        let weak = Rc::downgrade(self);
        let shown = text.to_owned();
        let id = glib::timeout_add_local_once(Duration::from_secs(4), move || {
            let Some(state) = weak.upgrade() else { return };
            state.toast_timer.borrow_mut().take();
            if state.w.status.label() == shown {
                state.notify("", false);
            }
        });
        *self.toast_timer.borrow_mut() = Some(id);
    }

    /// Shipping's `chooseDestinationDirectory`: the folder Save writes to.
    fn choose_folder(self: &Rc<Self>) {
        if self.saving.get() {
            return;
        }
        let window = self.w.preview.root().and_downcast::<gtk::Window>();
        #[allow(deprecated)]
        let chooser = gtk::FileChooserNative::new(
            Some("Choose save location"),
            window.as_ref(),
            gtk::FileChooserAction::SelectFolder,
            Some("Select"),
            Some("Cancel"),
        );
        #[allow(deprecated)]
        let _ = chooser.set_current_folder(Some(&gio::File::for_path(&*self.directory.borrow())));
        let weak = Rc::downgrade(self);
        #[allow(deprecated)]
        chooser.connect_response(move |chooser, response| {
            let folder = chooser.file().and_then(|file| file.path());
            chooser.destroy();
            let Some(state) = weak.upgrade() else { return };
            state.chooser.borrow_mut().take();
            if response == gtk::ResponseType::Accept
                && let Some(folder) = folder
            {
                *state.directory.borrow_mut() = folder;
                state.notify("", false);
                state.update_footer();
            }
        });
        #[allow(deprecated)]
        chooser.show();
        *self.chooser.borrow_mut() = Some(chooser);
    }

    fn saving_copy(&self) -> bool {
        self.original.borrow().is_none() || self.w.save_as_new.is_on()
    }

    fn destination(&self) -> PathBuf {
        let stem = self.w.filename.text();
        self.directory
            .borrow()
            .join(format!("{}{}", stem.trim(), self.output.extension()))
    }

    fn update_footer(&self) {
        let w = &self.w;
        w.folder
            .set_label(&format!("Saving to {}", self.directory.borrow().display()));
        let stem = w.filename.text();
        let error = ui::filename_error(&stem);
        // Shipping's `formatRequiresCopy`: another format is always a copy.
        let requires_copy = self.original.borrow().is_none()
            || self
                .output
                .source()
                .is_some_and(|source| source.format != self.output.format());
        if requires_copy && !w.save_as_new.is_on() {
            w.save_as_new.activate_toggle_quietly(true);
        }
        w.save_as_new.set_sensitive(!requires_copy);
        w.change_folder.set_sensitive(!self.saving.get());
        // Shipping keeps Cancel and Show in Folder's space as hidden
        // placeholders so the actions never shift.
        let saving = self.saving.get();
        let saved = self.saved.borrow().as_ref().map(|(_, print)| print.clone());
        let already_saved = !saving && saved.as_deref() == Some(self.fingerprint().as_str());
        for (button, shown) in [
            (&w.cancel, saving),
            (&w.show_in_folder, saved.is_some() && !saving),
        ] {
            button.set_child_visible(shown);
            button.set_sensitive(shown);
            button.set_can_target(shown);
            button.set_focusable(shown);
        }
        let valid = self.presented.borrow().as_ref().is_some_and(|p| {
            self.output
                .specs(p.edit.trim_start_ms, p.trim_end_opt())
                .is_some()
        });
        w.save.set_sensitive(
            error.is_none()
                && !already_saved
                && valid
                && !self.busy.get()
                && self.pending_apply.borrow().is_none(),
        );
        match error {
            Some(error) => self.notify(error, true),
            // `set_text` passes through an empty filename; clear its error
            // once the name is valid again, as shipping's `setError("")`.
            None if self.w.status.label() == ui::FILENAME_ERROR => self.notify("", false),
            None => {}
        }
    }

    fn save(&self) {
        let Some(export) = self
            .presented
            .borrow()
            .as_ref()
            .map(|presented| presented.save_export.clone())
        else {
            return;
        };
        if let Some(cancel) = self.playing.borrow_mut().take() {
            cancel.cancel();
        }
        if self.output.format() == VideoFormat::WebM {
            self.notify(ui::WEBM_EXPORT_ERROR, true);
            return;
        }
        self.cancel_estimate();
        self.busy.set(true);
        self.saving.set(true);
        self.update_footer();
        self.notify(
            ui::export_stage_label(captures_media::ExportStage::Preparing),
            false,
        );
        let cancel = CancelToken::default();
        *self.export_cancel.borrow_mut() = Some(cancel.clone());
        self.w.progress.set_fraction(0.);
        self.w.progress.set_visible(true);
        let destination = self.destination();
        let _ = self.jobs.send(if self.saving_copy() {
            Job::Save(
                RecordingSaveRequest {
                    destination,
                    export,
                },
                cancel,
            )
        } else {
            Job::Replace(destination, cancel)
        });
    }

    fn notify(&self, text: &str, error: bool) {
        self.w.status.set_label(text);
        self.w
            .status
            .update_property(&[gtk::accessible::Property::Label(text)]);
        set_class(&self.w.status, "error", error);
        // Progress stays subtle; only a toast is a success.
        set_class(&self.w.status, "success", false);
    }

    /// The crop box over the uncropped source, when both are ready: shipping
    /// plays and previews the source under its crop layer.
    fn crop_view(&self) -> Option<(CropRect, gdk::Texture)> {
        let crop = self.output.crop_shown()?;
        if self.playing.borrow().is_some() {
            return None;
        }
        let texture = self.source_frame.borrow().as_ref()?.1.clone();
        Some((crop, texture))
    }

    /// The crop handle (or the box itself) under a preview point.
    fn crop_handle_at(&self, x: f64, y: f64) -> Option<CropDragHandle> {
        let (crop, texture) = self.crop_view()?;
        let (bx, by, bw, _) = self.image_bounds.get();
        let scale = f64::from(bw) / f64::from(texture.width().max(1));
        let left = f64::from(bx) + f64::from(crop.x) * scale;
        let top = f64::from(by) + f64::from(crop.y) * scale;
        let right = left + f64::from(crop.width) * scale;
        let bottom = top + f64::from(crop.height) * scale;
        let (cx, cy) = ((left + right) / 2., (top + bottom) / 2.);
        use CropDragHandle::*;
        [
            (NorthWest, left, top),
            (NorthEast, right, top),
            (SouthEast, right, bottom),
            (SouthWest, left, bottom),
            (North, cx, top),
            (East, right, cy),
            (South, cx, bottom),
            (West, left, cy),
        ]
        .into_iter()
        .find(|(_, hx, hy)| (x - hx).abs() <= 8. && (y - hy).abs() <= 8.)
        .map(|(handle, _, _)| handle)
        .or_else(|| (x >= left && x <= right && y >= top && y <= bottom).then_some(Move))
    }

    fn draw_preview(&self, snapshot: &gtk::Snapshot, width: f32, height: f32) {
        let t = &self.tokens;
        snapshot.append_color(
            &t.rgba("surface-sunken"),
            &graphene::Rect::new(0., 0., width, height),
        );
        let crop_view = self.crop_view();
        let Some(texture) = crop_view
            .as_ref()
            .map(|(_, texture)| texture.clone())
            .or_else(|| self.frame.borrow().clone())
        else {
            return;
        };
        let pad = 12.;
        let (tw, th) = (texture.width() as f32, texture.height() as f32);
        let scale = if self.one_to_one.get() {
            1.
        } else {
            ((width - 2. * pad) / tw)
                .min((height - 2. * pad) / th)
                .max(0.01)
        };
        let (dw, dh) = (tw * scale, th * scale);
        let bounds = graphene::Rect::new((width - dw) / 2., (height - dh) / 2., dw, dh);
        snapshot.push_clip(&graphene::Rect::new(0., 0., width, height));
        let rounded = gsk::RoundedRect::from_rect(bounds, t.number("r-md"));
        snapshot.push_rounded_clip(&rounded);
        snapshot.append_texture(&texture, &bounds);
        snapshot.pop();
        snapshot.append_border(&rounded, &[1.; 4], &[t.rgba("border"); 4]);
        self.image_bounds.set((bounds.x(), bounds.y(), dw, dh));
        if let Some((crop, _)) = crop_view {
            self.draw_crop(snapshot, &bounds, scale, crop);
        }
        snapshot.pop();
    }

    /// Shipping `.editor-crop-layer`: dims, the accent box, its size badge
    /// and eight handles, as retained nodes over the source texture.
    fn draw_crop(
        &self,
        snapshot: &gtk::Snapshot,
        image: &graphene::Rect,
        scale: f32,
        crop: CropRect,
    ) {
        let t = &self.tokens;
        let accent = t.rgba("theme-accent");
        let ink = t.rgba("theme-accent-ink");
        let rect = graphene::Rect::new(
            image.x() + crop.x as f32 * scale,
            image.y() + crop.y as f32 * scale,
            crop.width as f32 * scale,
            crop.height as f32 * scale,
        );
        let dim = gdk::RGBA::new(3. / 255., 4. / 255., 6. / 255., 0.62);
        let (l, r) = (rect.x(), rect.x() + rect.width());
        let (top, bottom) = (rect.y(), rect.y() + rect.height());
        let (il, ir) = (image.x(), image.x() + image.width());
        let (it, ib) = (image.y(), image.y() + image.height());
        for dimmed in [
            graphene::Rect::new(il, it, ir - il, top - it),
            graphene::Rect::new(il, bottom, ir - il, ib - bottom),
            graphene::Rect::new(il, top, l - il, bottom - top),
            graphene::Rect::new(r, top, ir - r, bottom - top),
        ] {
            if dimmed.width() > 0. && dimmed.height() > 0. {
                snapshot.append_color(&dim, &dimmed);
            }
        }
        let outer = graphene::Rect::new(l - 1.5, top - 1.5, rect.width() + 3., rect.height() + 3.);
        snapshot.append_border(
            &gsk::RoundedRect::from_rect(outer, 0.),
            &[1.; 4],
            &[gdk::RGBA::new(0., 0., 0., 0.6); 4],
        );
        snapshot.append_border(
            &gsk::RoundedRect::from_rect(rect, 0.),
            &[1.5; 4],
            &[accent; 4],
        );
        // The size badge, centered at the box's top.
        let layout = self
            .w
            .preview
            .create_pango_layout(Some(&format!("{} × {}", crop.width, crop.height)));
        let mut font = gtk::pango::FontDescription::new();
        font.set_weight(gtk::pango::Weight::Semibold);
        font.set_absolute_size(f64::from(t.number("text-2xs")) * f64::from(gtk::pango::SCALE));
        layout.set_font_description(Some(&font));
        let (_, logical) = layout.pixel_extents();
        let (bw, bh) = (logical.width() as f32 + 16., logical.height() as f32 + 6.);
        let badge = graphene::Rect::new(l + (rect.width() - bw) / 2., top + 8., bw, bh);
        let rounded = gsk::RoundedRect::from_rect(badge, t.number("r-xs"));
        snapshot.push_rounded_clip(&rounded);
        snapshot.append_color(&accent, &badge);
        snapshot.pop();
        snapshot.save();
        snapshot.translate(&graphene::Point::new(badge.x() + 8., badge.y() + 3.));
        snapshot.append_layout(&layout, &ink);
        snapshot.restore();
        let (cx, cy) = ((l + r) / 2., (top + bottom) / 2.);
        for (hx, hy) in [
            (l, top),
            (cx, top),
            (r, top),
            (r, cy),
            (r, bottom),
            (cx, bottom),
            (l, bottom),
            (l, cy),
        ] {
            let handle =
                gsk::RoundedRect::from_rect(graphene::Rect::new(hx - 5.5, hy - 5.5, 11., 11.), 3.);
            snapshot.push_rounded_clip(&handle);
            snapshot.append_color(&accent, handle.bounds());
            snapshot.pop();
            snapshot.append_border(&handle, &[2.; 4], &[ink; 4]);
        }
    }

    fn draw_track(&self, snapshot: &gtk::Snapshot, _width: f32, height: f32) {
        let t = &self.tokens;
        let (left, track_width) = self.track_rect();
        let (left, track_width) = (left as f32, track_width as f32);
        // `.timeline-filmstrip`: inset by `--timeline-inset` on every side.
        let strip = graphene::Rect::new(left, left, track_width, height - 2. * left);
        let rounded = gsk::RoundedRect::from_rect(strip, t.number("r-xs"));
        snapshot.push_rounded_clip(&rounded);
        match self.filmstrip.borrow().clone() {
            Some(texture) => snapshot.append_texture(&texture, &strip),
            None => snapshot.append_color(&t.rgba("n-6"), &strip),
        }
        snapshot.pop();
        let duration = self.duration().max(1) as f32;
        let x = |time: f64| left + time as f32 / duration * track_width;
        let (start, end) = self.trim.get();
        // `.timeline-excluded`: a canvas-tinted veil outside the trim.
        let canvas = t.rgba("surface-canvas");
        let dark = canvas.red() + canvas.green() + canvas.blue() < 1.5;
        let veil = if dark {
            gdk::RGBA::new(5. / 255., 6. / 255., 8. / 255., 0.66)
        } else {
            gdk::RGBA::new(245. / 255., 245. / 255., 247. / 255., 0.76)
        };
        snapshot.append_color(&veil, &graphene::Rect::new(0., 0., x(start), height));
        let right = x(end);
        snapshot.append_color(
            &veil,
            &graphene::Rect::new(right, 0., left * 2. + track_width - right, height),
        );
        // `.timeline-playhead`: a 2 px text-colour line with a rounded tab.
        let head = x(self.playhead.get() as f64);
        let ink = t.rgba("text");
        snapshot.append_color(&ink, &graphene::Rect::new(head - 1., 0., 2., height));
        let tab = gsk::RoundedRect::new(
            graphene::Rect::new(head - 5., 0., 10., 6.),
            graphene::Size::new(3., 3.),
            graphene::Size::new(3., 3.),
            graphene::Size::new(1., 1.),
            graphene::Size::new(1., 1.),
        );
        snapshot.push_rounded_clip(&tab);
        snapshot.append_color(&ink, tab.bounds());
        snapshot.pop();
    }
}
