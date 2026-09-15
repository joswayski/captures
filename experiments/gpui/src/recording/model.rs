use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use captures_media::{AudioEdit, CropRect, EditSpec, ExportFormat, ExportSpec, QualityPreset};
use captures_recording::{
    AudioOptions, GifOptions, MaxResolution, RecordingKind, RecordingOptions, RecordingTarget,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActionMode {
    Screenshot,
    Recording,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetMode {
    Region,
    Window,
    Display,
}

/// Serializes asynchronous selector work. Tokens make stale countdowns and
/// delayed starts harmless after cancel, stop, or a newer recording request.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Lifecycle {
    generation: u64,
    countdown: bool,
    starting: bool,
    finalizing: bool,
}

impl Lifecycle {
    pub fn begin_countdown(&mut self) -> Option<u64> {
        if self.countdown || self.starting || self.finalizing {
            return None;
        }
        self.generation = self.generation.wrapping_add(1);
        self.countdown = true;
        Some(self.generation)
    }
    pub fn begin_start(&mut self, token: u64) -> bool {
        if token != self.generation || self.starting || self.finalizing {
            return false;
        }
        self.countdown = false;
        self.starting = true;
        true
    }
    pub fn started(&mut self, token: u64) -> bool {
        if token != self.generation || !self.starting {
            return false;
        }
        self.starting = false;
        true
    }
    pub fn begin_finalize(&mut self) -> bool {
        if self.finalizing {
            return false;
        }
        self.generation = self.generation.wrapping_add(1);
        self.countdown = false;
        self.starting = false;
        self.finalizing = true;
        true
    }
    pub fn finalized(&mut self) {
        self.finalizing = false;
    }
    pub fn cancel(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.countdown = false;
        self.starting = false;
    }
    pub fn current(&self, token: u64) -> bool {
        token == self.generation
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn from_drag(start: (f32, f32), end: (f32, f32), bounds: (f32, f32), square: bool) -> Self {
        let (mut dx, mut dy) = (end.0 - start.0, end.1 - start.1);
        if square {
            let side = dx.abs().min(dy.abs());
            dx = side.copysign(dx);
            dy = side.copysign(dy);
        }
        let x = start.0.min(start.0 + dx).clamp(0., bounds.0);
        let y = start.1.min(start.1 + dy).clamp(0., bounds.1);
        let right = start.0.max(start.0 + dx).clamp(0., bounds.0);
        let bottom = start.1.max(start.1 + dy).clamp(0., bounds.1);
        Self {
            x,
            y,
            width: right - x,
            height: bottom - y,
        }
    }
    pub fn valid(self) -> bool {
        self.width >= 2. && self.height >= 2.
    }

    pub fn handles(self) -> [(f32, f32); 8] {
        let (x, y, r, b) = (self.x, self.y, self.x + self.width, self.y + self.height);
        [
            (x, y),
            ((x + r) / 2., y),
            (r, y),
            (r, (y + b) / 2.),
            (r, b),
            ((x + r) / 2., b),
            (x, b),
            (x, (y + b) / 2.),
        ]
    }

    pub fn adjusted(self, handle: usize, delta: (f32, f32), bounds: (f32, f32)) -> Self {
        if handle == 8 {
            return Self {
                x: (self.x + delta.0).clamp(0., (bounds.0 - self.width).max(0.)),
                y: (self.y + delta.1).clamp(0., (bounds.1 - self.height).max(0.)),
                ..self
            };
        }
        let (mut x, mut y, mut r, mut b) =
            (self.x, self.y, self.x + self.width, self.y + self.height);
        if matches!(handle, 0 | 6 | 7) {
            x = (x + delta.0).clamp(0., r - 2.);
        }
        if matches!(handle, 0..=2) {
            y = (y + delta.1).clamp(0., b - 2.);
        }
        if matches!(handle, 2..=4) {
            r = (r + delta.0).clamp(x + 2., bounds.0);
        }
        if matches!(handle, 4..=6) {
            b = (b + delta.1).clamp(y + 2., bounds.1);
        }
        Self {
            x,
            y,
            width: r - x,
            height: b - y,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub video_fps: u16,
    pub video_max_resolution: MaxResolution,
    pub gif_fps: u16,
    pub gif_max_width: u32,
    pub gif_max_colors: u16,
    pub countdown_seconds: u8,
    pub show_cursor: bool,
    pub capture_system_audio: bool,
    pub microphone_device_id: Option<String>,
    pub mono_audio: bool,
    pub highlight_clicks: bool,
    pub show_keystrokes: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            video_fps: 60,
            video_max_resolution: MaxResolution::Original,
            gif_fps: 15,
            gif_max_width: 800,
            gif_max_colors: 256,
            countdown_seconds: 3,
            show_cursor: true,
            capture_system_audio: false,
            microphone_device_id: None,
            mono_audio: false,
            highlight_clicks: false,
            show_keystrokes: false,
        }
    }
}
#[derive(Deserialize, Default)]
struct SettingsFile {
    #[serde(default)]
    recording: Settings,
}
impl Settings {
    pub fn load(profile: &Path) -> Self {
        fs::read(profile.join("settings.json"))
            .ok()
            .and_then(|v| serde_json::from_slice::<SettingsFile>(&v).ok())
            .map(|v| v.recording)
            .unwrap_or_default()
    }
    pub fn options(&self, kind: RecordingKind, target: RecordingTarget) -> RecordingOptions {
        RecordingOptions {
            kind,
            target,
            frames_per_second: if kind == RecordingKind::Gif {
                self.gif_fps
            } else {
                self.video_fps
            },
            max_resolution: self.video_max_resolution,
            countdown_seconds: self.countdown_seconds,
            show_cursor: self.show_cursor,
            highlight_clicks: self.highlight_clicks,
            show_keystrokes: self.show_keystrokes,
            audio: if kind == RecordingKind::Gif {
                AudioOptions::default()
            } else {
                AudioOptions {
                    capture_system_audio: self.capture_system_audio,
                    microphone_device_id: self.microphone_device_id.clone(),
                    mono_output: self.mono_audio,
                    ..Default::default()
                }
            },
            gif: GifOptions {
                max_width: self.gif_max_width,
                max_colors: self.gif_max_colors,
                optimize: true,
            },
        }
    }
}

pub fn timestamped(captures: &Path, extension: &str) -> PathBuf {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    captures.join(format!("Captures_{millis}.{extension}"))
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EditorState {
    pub duration_ms: u64,
    pub trim_start_ms: u64,
    pub trim_end_ms: u64,
    pub crop: Option<CropRect>,
    pub output_width: Option<u32>,
    pub output_height: Option<u32>,
    pub format: ExportFormat,
    pub quality: QualityPreset,
    pub max_size_bytes: Option<u64>,
    pub gif_fps: u16,
    pub gif_colors: u16,
    pub system_volume: f32,
    pub microphone_volume: f32,
    pub mute_system_audio: bool,
    pub mute_microphone: bool,
    pub mono_audio: bool,
    pub playback_rate: f32,
}
impl EditorState {
    pub fn new(duration_ms: u64, format: ExportFormat) -> Self {
        Self {
            duration_ms,
            trim_start_ms: 0,
            trim_end_ms: duration_ms,
            crop: None,
            output_width: None,
            output_height: None,
            format,
            quality: QualityPreset::Preserve,
            max_size_bytes: None,
            gif_fps: 15,
            gif_colors: 256,
            system_volume: 1.0,
            microphone_volume: 1.0,
            mute_system_audio: false,
            mute_microphone: false,
            mono_audio: false,
            playback_rate: 1.0,
        }
    }
    pub fn edit(&self, has_audio: bool) -> EditSpec {
        let (start, end) =
            ordered_trim_bounds(self.trim_start_ms, self.trim_end_ms, self.duration_ms);
        EditSpec {
            trim_start_ms: start,
            trim_end_ms: Some(end),
            crop: self.crop,
            output_width: self.output_width,
            output_height: self.output_height,
            audio: AudioEdit {
                system_volume: self.system_volume.clamp(0., 2.),
                microphone_volume: self.microphone_volume.clamp(0., 2.),
                mute_system_audio: self.mute_system_audio,
                mute_microphone: self.mute_microphone,
                mono_output: self.mono_audio,
                source_has_system_audio: has_audio,
                ..Default::default()
            },
        }
    }
    pub fn export(&self) -> ExportSpec {
        ExportSpec {
            format: self.format,
            quality: self.quality,
            max_size_bytes: self.max_size_bytes,
            frames_per_second: (self.format == ExportFormat::Gif).then_some(self.gif_fps),
            gif_max_colors: (self.format == ExportFormat::Gif).then_some(self.gif_colors),
        }
    }

    pub fn set_crop(&mut self, crop: Option<CropRect>, source_width: u32, source_height: u32) {
        self.crop = crop.map(|crop| {
            let x = crop.x.min(source_width.saturating_sub(2));
            let y = crop.y.min(source_height.saturating_sub(2));
            CropRect {
                x,
                y,
                width: crop.width.max(2).min(source_width.saturating_sub(x)),
                height: crop.height.max(2).min(source_height.saturating_sub(y)),
            }
        });
    }
}

/// Normalize both handles symmetrically. A reversed drag swaps the handles;
/// the one-millisecond minimum is applied only after clamping to the source.
pub fn ordered_trim_bounds(start: u64, end: u64, duration: u64) -> (u64, u64) {
    let duration = duration.max(1);
    let (mut start, mut end) = (start.min(end).min(duration), start.max(end).min(duration));
    if start == end {
        if end < duration {
            end += 1;
        } else {
            start = start.saturating_sub(1);
        }
    }
    (start, end)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drag_is_normalized_clamped_and_shift_square() {
        assert_eq!(
            Rect::from_drag((90., 80.), (10., -20.), (100., 100.), true),
            Rect {
                x: 10.,
                y: 0.,
                width: 80.,
                height: 80.
            }
        );
    }
    #[test]
    fn trim_is_always_ordered_and_bounded() {
        let mut s = EditorState::new(1000, ExportFormat::Mp4);
        s.trim_start_ms = 900;
        s.trim_end_ms = 100;
        let e = s.edit(false);
        assert_eq!((e.trim_start_ms, e.trim_end_ms), (100, Some(900)));
    }
    #[test]
    fn equal_and_out_of_range_trim_handles_keep_one_millisecond() {
        assert_eq!(ordered_trim_bounds(2_000, 2_000, 1_000), (999, 1_000));
        assert_eq!(ordered_trim_bounds(0, 0, 1_000), (0, 1));
    }
    #[test]
    fn gif_never_inherits_audio_capture() {
        let s = Settings {
            capture_system_audio: true,
            ..Default::default()
        };
        assert!(
            !s.options(
                RecordingKind::Gif,
                RecordingTarget::Display {
                    display_id: "x".into()
                }
            )
            .audio
            .capture_system_audio
        );
    }
    #[test]
    fn lifecycle_rejects_duplicate_and_stale_async_work() {
        let mut lifecycle = Lifecycle::default();
        let first = lifecycle.begin_countdown().unwrap();
        assert!(lifecycle.begin_countdown().is_none());
        lifecycle.cancel();
        assert!(!lifecycle.begin_start(first));
        let second = lifecycle.begin_countdown().unwrap();
        assert!(lifecycle.begin_start(second));
        assert!(!lifecycle.begin_start(second));
        assert!(lifecycle.started(second));
    }

    #[test]
    fn finalization_invalidates_delayed_start_and_is_single_flight() {
        let mut lifecycle = Lifecycle::default();
        let token = lifecycle.begin_countdown().unwrap();
        assert!(lifecycle.begin_finalize());
        assert!(!lifecycle.begin_finalize());
        assert!(!lifecycle.current(token));
        lifecycle.finalized();
        assert!(lifecycle.begin_countdown().is_some());
    }
}
