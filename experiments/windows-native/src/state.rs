use crate::{
    editor::{Document, Tool},
    geometry::{Point, Rect, SelectionDrag},
    history::Artifact,
};
use captures_capture::{CaptureMode, DisplayDescriptor, DisplayFrame, WindowDescriptor};
use captures_media::QualityPreset;
use captures_recording::{RecordingKind, RecordingState};
use image::RgbaImage;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub fn sanitize_editor_filename(value: &str) -> String {
    let value = value
        .chars()
        .filter(|character| {
            !matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            )
        })
        .collect::<String>();
    let value = value.trim().trim_end_matches(['.', ' ']);
    if value.is_empty() {
        "Capture".into()
    } else {
        value.chars().take(120).collect()
    }
}

pub fn source_image_format(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => Some("jpeg"),
        "png" => Some("png"),
        "webp" => Some("webp"),
        _ => None,
    }
}

pub fn can_replace_editor_source(source: Option<&Path>, format: &str, filename: &str) -> bool {
    let Some(source) = source else {
        return false;
    };
    let filename = sanitize_editor_filename(filename);
    source_image_format(source) == Some(format)
        && source.file_stem().and_then(|value| value.to_str()) == Some(filename.as_str())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Surface {
    Menu,
    Overlay,
    ScreenshotEditor,
    RecordingSelector,
    RecordingHud,
    RecordingEditor,
    Preview,
    History,
    Preferences,
    DeleteConfirmation,
}

#[derive(Clone, Debug)]
pub struct Preview {
    pub artifact: Artifact,
    pub image: RgbaImage,
    pub appeared: Instant,
    pub dismissing: Option<Instant>,
}

#[derive(Clone, Debug)]
pub struct RecordingUi {
    pub state: RecordingState,
    pub kind: RecordingKind,
    pub started: Instant,
    pub elapsed_before_pause: Duration,
    pub muted: bool,
    pub hidden: bool,
    pub warning: Option<String>,
    pub source: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct RecordingEditorState {
    pub source: PathBuf,
    pub duration_ms: u64,
    pub width: u32,
    pub height: u32,
    pub position_ms: u64,
    pub trim_start_ms: u64,
    pub trim_end_ms: u64,
    pub playing: bool,
    pub quality: QualityPreset,
    pub quality_menu_open: bool,
    pub save_as_new: bool,
    pub has_audio: bool,
    pub comparison_estimated_bytes: Option<u64>,
    clock: Option<(Instant, u64)>,
}

impl RecordingEditorState {
    pub fn new(
        source: PathBuf,
        duration_ms: u64,
        width: u32,
        height: u32,
        has_audio: bool,
    ) -> Result<Self, &'static str> {
        if duration_ms == 0 || width == 0 || height == 0 {
            return Err("recording duration is unavailable");
        }
        Ok(Self {
            source,
            duration_ms,
            width,
            height,
            position_ms: 0,
            trim_start_ms: 0,
            trim_end_ms: duration_ms,
            playing: false,
            quality: QualityPreset::Highest,
            quality_menu_open: false,
            save_as_new: true,
            has_audio,
            comparison_estimated_bytes: None,
            clock: None,
        })
    }

    pub fn seek(&mut self, position_ms: u64) {
        self.position_ms = position_ms.clamp(self.trim_start_ms, self.trim_end_ms);
        if self.playing {
            self.clock = Some((Instant::now(), self.position_ms));
        }
    }

    pub fn set_trim_start(&mut self, position_ms: u64) {
        self.trim_start_ms = position_ms.min(self.trim_end_ms.saturating_sub(1));
        self.seek(self.position_ms);
    }

    pub fn set_trim_end(&mut self, position_ms: u64) {
        self.trim_end_ms =
            position_ms.clamp(self.trim_start_ms.saturating_add(1), self.duration_ms);
        self.seek(self.position_ms);
    }

    pub fn toggle_playback(&mut self, now: Instant) {
        if !self.playing && self.position_ms >= self.trim_end_ms {
            self.position_ms = self.trim_start_ms;
        }
        self.playing = !self.playing;
        self.clock = self.playing.then_some((now, self.position_ms));
    }

    pub fn tick(&mut self, now: Instant) -> bool {
        let Some((started, initial_ms)) = self.clock else {
            return false;
        };
        let next =
            initial_ms.saturating_add(now.saturating_duration_since(started).as_millis() as u64);
        if next >= self.trim_end_ms {
            self.position_ms = self.trim_end_ms;
            self.playing = false;
            self.clock = None;
        } else {
            self.position_ms = next;
        }
        true
    }
}

impl RecordingUi {
    pub fn elapsed(&self, now: Instant) -> Duration {
        self.elapsed_before_pause
            + if self.state == RecordingState::Recording {
                now.saturating_duration_since(self.started)
            } else {
                Duration::ZERO
            }
    }

    pub fn transition(
        &mut self,
        next: RecordingState,
        session_available: bool,
    ) -> Result<(), &'static str> {
        if !session_available
            && matches!(next, RecordingState::Recording | RecordingState::Countdown)
        {
            return Err("desktop session unavailable");
        }
        let valid = matches!(
            (self.state, next),
            (
                RecordingState::Selecting,
                RecordingState::Countdown | RecordingState::Recording | RecordingState::Discarded
            ) | (
                RecordingState::Countdown,
                RecordingState::Recording | RecordingState::Discarded
            ) | (
                RecordingState::Recording,
                RecordingState::Paused
                    | RecordingState::Finalizing
                    | RecordingState::Discarded
                    | RecordingState::Failed
            ) | (
                RecordingState::Paused,
                RecordingState::Recording
                    | RecordingState::Finalizing
                    | RecordingState::Discarded
                    | RecordingState::Failed
            ) | (
                RecordingState::Finalizing,
                RecordingState::Ready | RecordingState::Editor | RecordingState::Failed
            ) | (
                RecordingState::Editor,
                RecordingState::Ready | RecordingState::Discarded
            )
        );
        if !valid {
            return Err("invalid recording state transition");
        }
        if self.state == RecordingState::Recording && next == RecordingState::Paused {
            self.elapsed_before_pause += self.started.elapsed();
        }
        if self.state == RecordingState::Paused && next == RecordingState::Recording {
            self.started = Instant::now();
        }
        self.state = next;
        Ok(())
    }
}

pub struct OverlayState {
    pub mode: CaptureMode,
    pub frame: DisplayFrame,
    pub windows: Vec<WindowDescriptor>,
    pub hovered_window: Option<usize>,
    pub selection: Option<Rect>,
    pub drag: Option<SelectionDrag>,
    pub recording: bool,
}

pub struct AppState {
    pub surface: Surface,
    pub overlay: Option<OverlayState>,
    pub editor: Option<Document>,
    pub editor_tool: Tool,
    pub selected_layer: Option<u64>,
    pub editor_color: [u8; 4],
    pub editor_color_hex: String,
    pub editor_stroke: f32,
    pub editor_fill: Option<[u8; 4]>,
    pub editor_editing_color: bool,
    pub editor_source: Option<PathBuf>,
    pub editor_filename: String,
    pub editor_editing_filename: bool,
    pub editor_format: String,
    pub editor_save_as_new: bool,
    pub editor_export_settings_open: bool,
    pub editor_shapes_open: bool,
    pub previews: Vec<Preview>,
    pub recording: Option<RecordingUi>,
    pub pending_delete: Option<Artifact>,
    pub status: Option<(String, Instant)>,
    pub display: Option<DisplayDescriptor>,
    pub recording_preview: Option<RgbaImage>,
    pub recording_editor: Option<RecordingEditorState>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            surface: Surface::Menu,
            overlay: None,
            editor: None,
            editor_tool: Tool::Select,
            selected_layer: None,
            editor_color: [239, 70, 80, 255],
            editor_color_hex: "#ef4650".into(),
            editor_stroke: 3.0,
            editor_fill: None,
            editor_editing_color: false,
            editor_source: None,
            editor_filename: "Capture".into(),
            editor_editing_filename: false,
            editor_format: "png".into(),
            editor_save_as_new: true,
            editor_export_settings_open: false,
            editor_shapes_open: false,
            previews: Vec::new(),
            recording: None,
            pending_delete: None,
            status: None,
            display: None,
            recording_preview: None,
            recording_editor: None,
        }
    }
}

impl AppState {
    pub fn begin_overlay(
        &mut self,
        mode: CaptureMode,
        frame: DisplayFrame,
        windows: Vec<WindowDescriptor>,
        recording: bool,
    ) {
        self.display = Some(frame.descriptor.clone());
        self.overlay = Some(OverlayState {
            mode,
            frame,
            windows,
            hovered_window: None,
            selection: None,
            drag: None,
            recording,
        });
        self.surface = Surface::Overlay;
    }

    pub fn cancel_overlay(&mut self) {
        self.overlay = None;
        self.surface = Surface::Menu;
    }

    pub fn edit_image(&mut self, image: RgbaImage) {
        self.edit_image_from(image, None);
    }

    pub fn edit_image_from(&mut self, image: RgbaImage, source: Option<PathBuf>) {
        let filename = source
            .as_ref()
            .and_then(|path| path.file_stem())
            .and_then(|value| value.to_str())
            .unwrap_or("Capture")
            .to_owned();
        let format = source
            .as_deref()
            .and_then(source_image_format)
            .unwrap_or("png")
            .to_owned();
        let save_as_new = !can_replace_editor_source(source.as_deref(), &format, &filename);
        self.editor = Some(Document::new(image));
        self.selected_layer = None;
        self.editor_filename = filename;
        self.editor_editing_filename = false;
        self.editor_format = format;
        self.editor_save_as_new = save_as_new;
        self.editor_source = source;
        self.editor_export_settings_open = false;
        self.editor_shapes_open = false;
        self.surface = Surface::ScreenshotEditor;
    }

    pub fn add_preview(&mut self, artifact: Artifact, image: RgbaImage, now: Instant) {
        self.previews
            .retain(|preview| preview.artifact.path != artifact.path);
        self.previews.insert(
            0,
            Preview {
                artifact,
                image,
                appeared: now,
                dismissing: None,
            },
        );
        self.previews.truncate(5);
    }

    pub fn tick(&mut self, now: Instant) {
        self.previews.retain(|preview| {
            preview.dismissing.is_none_or(|start| {
                now.saturating_duration_since(start) < Duration::from_millis(420)
            })
        });
        if self.status.as_ref().is_some_and(|(_, start)| {
            now.saturating_duration_since(*start) > Duration::from_secs(4)
        }) {
            self.status = None;
        }
    }

    pub fn hit_window(&self, point: Point) -> Option<usize> {
        let overlay = self.overlay.as_ref()?;
        overlay.windows.iter().position(|window| {
            let scale = overlay.frame.descriptor.scale_factor.max(1.0) as f32;
            Rect {
                x: (window.x - overlay.frame.descriptor.x) as f32 / scale,
                y: (window.y - overlay.frame.descriptor.y) as f32 / scale,
                width: window.width as f32 / scale,
                height: window.height as f32 / scale,
            }
            .contains(point)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recording_cannot_resume_when_fail_closed_session_check_fails() {
        let mut ui = RecordingUi {
            state: RecordingState::Paused,
            kind: RecordingKind::Video,
            started: Instant::now(),
            elapsed_before_pause: Duration::from_secs(3),
            muted: false,
            hidden: false,
            warning: None,
            source: None,
        };
        assert_eq!(
            ui.transition(RecordingState::Recording, false),
            Err("desktop session unavailable")
        );
        assert_eq!(ui.state, RecordingState::Paused);
    }

    #[test]
    fn illegal_ready_to_recording_transition_is_rejected() {
        let mut ui = RecordingUi {
            state: RecordingState::Ready,
            kind: RecordingKind::Video,
            started: Instant::now(),
            elapsed_before_pause: Duration::ZERO,
            muted: false,
            hidden: false,
            warning: None,
            source: None,
        };
        assert_eq!(
            ui.transition(RecordingState::Recording, true),
            Err("invalid recording state transition")
        );
    }

    #[test]
    fn recording_editor_trim_and_seek_remain_inside_asymmetric_range() {
        let mut editor =
            RecordingEditorState::new("source.mp4".into(), 9_123, 1280, 720, true).unwrap();
        editor.set_trim_start(2_345);
        editor.set_trim_end(7_654);
        editor.seek(100);
        assert_eq!(editor.position_ms, 2_345);
        editor.seek(9_000);
        assert_eq!(editor.position_ms, 7_654);
        editor.set_trim_start(8_000);
        assert_eq!(editor.trim_start_ms, 7_653);
        editor.set_trim_end(1_000);
        assert_eq!(editor.trim_end_ms, 7_654);
    }

    #[test]
    fn existing_jpg_editor_preserves_filename_and_replace_option() {
        let mut state = AppState::default();
        state.edit_image_from(
            RgbaImage::new(3, 7),
            Some(PathBuf::from("/Captures/uneven-name.JPG")),
        );
        assert_eq!(state.editor_filename, "uneven-name");
        assert_eq!(state.editor_format, "jpeg");
        assert!(!state.editor_save_as_new);
        assert_eq!(
            state.editor_source,
            Some(PathBuf::from("/Captures/uneven-name.JPG"))
        );
        assert!(can_replace_editor_source(
            state.editor_source.as_deref(),
            &state.editor_format,
            &state.editor_filename
        ));
    }

    #[test]
    fn format_divergence_forces_new_file() {
        let source = Path::new("/Captures/original.jpg");
        assert!(!can_replace_editor_source(Some(source), "png", "original"));
    }

    #[test]
    fn filename_divergence_forces_new_file() {
        let source = Path::new("/Captures/original.png");
        assert!(!can_replace_editor_source(Some(source), "png", "renamed"));
    }

    #[test]
    fn unsaved_image_cannot_replace_a_source() {
        assert!(!can_replace_editor_source(None, "png", "Capture"));
    }

    #[test]
    fn unsupported_source_extension_cannot_be_replaced_as_png() {
        let source = Path::new("/Captures/original.bmp");
        assert!(!can_replace_editor_source(Some(source), "png", "original"));
    }
}
