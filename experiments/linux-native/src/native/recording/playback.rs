//! Lightweight in-window playback for the GTK experiment.
//!
//! GTK3 has no media widget and this experiment intentionally does not add a
//! webview. FFmpeg decodes frames into the GTK image while one headless ffplay
//! process per source audio stream keeps system and microphone controls
//! independent. The child processes never create a player window.
use crate::compat::prelude::*;
use captures_media::{CancelToken, MediaToolchain};
use gtk::{glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    process::{Child, Command, Stdio},
    rc::Rc,
    time::{Duration, Instant},
};

use crate::ui;

pub struct Playback {
    path: PathBuf,
    image: gtk::Image,
    position: gtk::Scale,
    play: gtk::Button,
    status: gtk::Label,
    duration_ms: Rc<Cell<u64>>,
    trim_start_ms: Rc<Cell<u64>>,
    trim_end_ms: Rc<Cell<u64>>,
    playing: Cell<bool>,
    looping: Cell<bool>,
    clock: RefCell<Option<(Instant, u64)>>,
    generation: Cell<u64>,
    frame_busy: Cell<bool>,
    children: RefCell<Vec<Child>>,
    closed: Rc<Cell<bool>>,
    scratch: PathBuf,
    audio: RefCell<AudioPlayback>,
}

#[derive(Clone, Copy)]
pub struct AudioPlayback {
    pub system_stream: bool,
    pub microphone_stream: bool,
    pub system_volume: f64,
    pub microphone_volume: f64,
    pub mute_system: bool,
    pub mute_microphone: bool,
}

impl Playback {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        path: PathBuf,
        image: gtk::Image,
        position: gtk::Scale,
        play: gtk::Button,
        status: gtk::Label,
        duration_ms: Rc<Cell<u64>>,
        trim_start_ms: Rc<Cell<u64>>,
        trim_end_ms: Rc<Cell<u64>>,
        closed: Rc<Cell<bool>>,
        scratch: PathBuf,
        audio: AudioPlayback,
    ) -> Rc<Self> {
        let playback = Rc::new(Self {
            path,
            image,
            position,
            play,
            status,
            duration_ms,
            trim_start_ms,
            trim_end_ms,
            playing: Cell::new(false),
            looping: Cell::new(false),
            clock: RefCell::new(None),
            generation: Cell::new(0),
            frame_busy: Cell::new(false),
            children: RefCell::new(Vec::new()),
            closed,
            scratch,
            audio: RefCell::new(audio),
        });
        let weak = Rc::downgrade(&playback);
        glib::timeout_add_local(Duration::from_millis(40), move || {
            let Some(playback) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if playback.closed.get() {
                playback.pause();
                return glib::ControlFlow::Break;
            }
            playback.tick();
            glib::ControlFlow::Continue
        });
        playback
    }

    pub fn toggle(self: &Rc<Self>) {
        if self.playing.get() {
            self.pause();
        } else {
            self.start();
        }
    }

    pub fn seek(self: &Rc<Self>, at_ms: u64) {
        let at_ms = at_ms.clamp(self.trim_start_ms.get(), self.trim_end_ms.get());
        self.position.set_value(at_ms as f64);
        self.request_frame(at_ms);
        if self.playing.get() {
            self.restart_clock_and_audio(at_ms);
        }
    }

    pub fn set_audio(&self, audio: AudioPlayback) {
        *self.audio.borrow_mut() = audio;
        if self.playing.get() {
            self.restart_clock_and_audio(self.position.value() as u64);
        }
    }

    pub fn set_looping(&self, looping: bool) {
        self.looping.set(looping);
    }

    pub fn request_frame(self: &Rc<Self>, at_ms: u64) {
        if self.frame_busy.replace(true) || self.closed.get() {
            return;
        }
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        let output = self.scratch.join(format!("playback-{generation}.png"));
        let path = self.path.clone();
        let weak = Rc::downgrade(self);
        ui::job(
            move || {
                MediaToolchain::from_command_names()
                    .extract_frame(&path, at_ms, &output, &CancelToken::default())
                    .map_err(|error| error.to_string())?;
                Ok(output)
            },
            move |result| {
                let Some(playback) = weak.upgrade() else {
                    if let Ok(path) = result {
                        let _ = std::fs::remove_file(path);
                    }
                    return;
                };
                playback.frame_busy.set(false);
                if let Ok(path) = result {
                    if !playback.closed.get()
                        && playback.generation.get() == generation
                        && let Ok(frame) = gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(
                            &path,
                            playback.image.allocated_width().max(2),
                            playback.image.allocated_height().max(2),
                            true,
                        )
                    {
                        playback.image.set_from_pixbuf(Some(&frame));
                    }
                    let _ = std::fs::remove_file(path);
                }
            },
        );
    }

    pub fn pause(&self) {
        self.playing.set(false);
        self.play.set_image(Some(&ui::icon("play", 18)));
        self.play.set_always_show_image(true);
        ui::named(&self.play, "Play preview");
        *self.clock.borrow_mut() = None;
        stop_children(&mut self.children.borrow_mut());
    }

    fn start(self: &Rc<Self>) {
        let mut at_ms = self.position.value().max(0.0) as u64;
        if at_ms >= self.trim_end_ms.get() {
            at_ms = self.trim_start_ms.get();
            self.position.set_value(at_ms as f64);
        }
        self.playing.set(true);
        self.play.set_image(Some(&ui::icon("pause", 18)));
        self.play.set_always_show_image(true);
        ui::named(&self.play, "Pause preview");
        self.restart_clock_and_audio(at_ms);
    }

    fn restart_clock_and_audio(&self, at_ms: u64) {
        stop_children(&mut self.children.borrow_mut());
        *self.clock.borrow_mut() = Some((Instant::now(), at_ms));
        let audio = *self.audio.borrow();
        let mut stream_index = 0;
        if audio.system_stream {
            if !audio.mute_system && audio.system_volume > 0.0 {
                self.spawn_audio(stream_index, at_ms, audio.system_volume);
            }
            stream_index += 1;
        }
        if audio.microphone_stream && !audio.mute_microphone && audio.microphone_volume > 0.0 {
            self.spawn_audio(stream_index, at_ms, audio.microphone_volume);
        }
    }

    fn spawn_audio(&self, stream_index: usize, at_ms: u64, volume: f64) {
        let remaining = self.trim_end_ms.get().saturating_sub(at_ms);
        let child = Command::new("ffplay")
            .args(["-nodisp", "-autoexit", "-loglevel", "error", "-ss"])
            .arg(format!("{:.3}", at_ms as f64 / 1_000.0))
            .arg("-t")
            .arg(format!("{:.3}", remaining as f64 / 1_000.0))
            .arg("-volume")
            .arg((volume * 100.0).round().clamp(0.0, 200.0).to_string())
            .arg("-ast")
            .arg(stream_index.to_string())
            .arg(&self.path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        match child {
            Ok(child) => self.children.borrow_mut().push(child),
            Err(error) => self
                .status
                .set_text(&format!("Video is playing without audio: {error}")),
        }
    }

    fn tick(self: &Rc<Self>) {
        if !self.playing.get() {
            return;
        }
        let Some((started, initial_ms)) = *self.clock.borrow() else {
            return;
        };
        let at_ms = initial_ms.saturating_add(started.elapsed().as_millis() as u64);
        if at_ms >= self.trim_end_ms.get().min(self.duration_ms.get()) {
            if self.looping.get() {
                let start = self.trim_start_ms.get();
                self.position.set_value(start as f64);
                self.restart_clock_and_audio(start);
                self.request_frame(start);
                return;
            }
            self.position.set_value(self.trim_end_ms.get() as f64);
            self.pause();
            self.request_frame(self.trim_end_ms.get().saturating_sub(1));
            return;
        }
        self.position.set_value(at_ms as f64);
        if !self.frame_busy.get() {
            self.request_frame(at_ms);
        }
    }
}

fn stop_children(children: &mut Vec<Child>) {
    for child in children.iter_mut() {
        let _ = child.kill();
        let _ = child.wait();
    }
    children.clear();
}
