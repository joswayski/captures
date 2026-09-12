use captures_media::{AudioEdit, CropRect, EditSpec, ExportFormat, ExportSpec, QualityPreset};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CropHandle {
    Move,
    NorthWest,
    NorthEast,
    SouthWest,
    SouthEast,
}

pub(super) fn crop_after_drag(
    initial: CropRect,
    handle: CropHandle,
    delta_x: i32,
    delta_y: i32,
    source_width: u32,
    source_height: u32,
    aspect_locked: bool,
) -> CropRect {
    if handle == CropHandle::Move {
        return CropRect {
            x: (i64::from(initial.x) + i64::from(delta_x))
                .clamp(0, i64::from(source_width.saturating_sub(initial.width)))
                as u32,
            y: (i64::from(initial.y) + i64::from(delta_y))
                .clamp(0, i64::from(source_height.saturating_sub(initial.height)))
                as u32,
            ..initial
        };
    }
    let mut left = i64::from(initial.x);
    let mut top = i64::from(initial.y);
    let mut right = left + i64::from(initial.width);
    let mut bottom = top + i64::from(initial.height);
    if matches!(handle, CropHandle::NorthWest | CropHandle::SouthWest) {
        left = (left + i64::from(delta_x)).clamp(0, right - 2);
    } else {
        right = (right + i64::from(delta_x)).clamp(left + 2, i64::from(source_width));
    }
    if matches!(handle, CropHandle::NorthWest | CropHandle::NorthEast) {
        top = (top + i64::from(delta_y)).clamp(0, bottom - 2);
    } else {
        bottom = (bottom + i64::from(delta_y)).clamp(top + 2, i64::from(source_height));
    }
    if aspect_locked {
        let ratio = f64::from(initial.width) / f64::from(initial.height.max(1));
        let width = (right - left) as f64;
        let height = (bottom - top) as f64;
        let horizontal_change = (width - f64::from(initial.width)).abs();
        let vertical_change = (height - f64::from(initial.height)).abs() * ratio;
        let requested_scale = if horizontal_change >= vertical_change {
            width / f64::from(initial.width.max(1))
        } else {
            height / f64::from(initial.height.max(1))
        };
        let west = matches!(handle, CropHandle::NorthWest | CropHandle::SouthWest);
        let north = matches!(handle, CropHandle::NorthWest | CropHandle::NorthEast);
        let available_width = if west {
            right
        } else {
            i64::from(source_width) - left
        };
        let available_height = if north {
            bottom
        } else {
            i64::from(source_height) - top
        };
        let maximum_scale = (available_width as f64 / f64::from(initial.width.max(1)))
            .min(available_height as f64 / f64::from(initial.height.max(1)));
        let minimum_scale =
            (2.0 / f64::from(initial.width.max(1))).max(2.0 / f64::from(initial.height.max(1)));
        let scale = requested_scale.clamp(minimum_scale, maximum_scale.max(minimum_scale));
        let corrected_width = (f64::from(initial.width) * scale).round().max(2.0) as i64;
        let corrected_height = (f64::from(initial.height) * scale).round().max(2.0) as i64;
        if west {
            left = right - corrected_width;
        } else {
            right = left + corrected_width;
        }
        if north {
            top = bottom - corrected_height;
        } else {
            bottom = top + corrected_height;
        }
    }
    CropRect {
        x: left as u32,
        y: top as u32,
        width: (right - left).max(2) as u32,
        height: (bottom - top).max(2) as u32,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HudPhase {
    Countdown(u8),
    Starting,
    Recording,
    Pausing,
    Paused,
    Finalizing,
    Failed,
    Discarding,
}

impl HudPhase {
    pub(super) fn controllable(self) -> bool {
        matches!(self, Self::Recording | Self::Paused | Self::Failed)
    }

    pub(super) fn status(self) -> &'static str {
        match self {
            Self::Countdown(_) | Self::Starting => "Starting…",
            Self::Recording => "Recording",
            Self::Pausing => "Pausing…",
            Self::Paused => "Paused",
            Self::Finalizing => "Saving…",
            Self::Failed => "Failed",
            Self::Discarding => "Deleting…",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PendingAction {
    None,
    Pause,
    Resume,
    Screenshot { resume_after: bool },
    Restart,
    Stop,
    Discard,
    LockPause,
    MuteRestart,
}

#[derive(Clone, Debug)]
pub(super) struct EditorSettings {
    pub trim_start_ms: u64,
    pub trim_end_ms: u64,
    pub crop_enabled: bool,
    pub crop: CropRect,
    pub aspect_locked: bool,
    pub resolution: Resolution,
    pub format: ExportFormat,
    pub quality: QualityPreset,
    pub maximum_size_bytes: Option<u64>,
    pub gif_fps: u16,
    pub gif_max_width: u32,
    pub system_volume: u16,
    pub microphone_volume: u16,
    pub mute_system: bool,
    pub mute_microphone: bool,
    pub mono: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Resolution {
    Original,
    P1080,
    P720,
    Half,
}

impl EditorSettings {
    pub(super) fn new(
        width: u32,
        height: u32,
        duration_ms: u64,
        has_system_audio: bool,
        has_microphone_audio: bool,
    ) -> Self {
        Self {
            trim_start_ms: 0,
            trim_end_ms: duration_ms.max(1),
            crop_enabled: false,
            crop: CropRect {
                x: 0,
                y: 0,
                width,
                height,
            },
            aspect_locked: true,
            resolution: Resolution::Original,
            format: ExportFormat::Mp4,
            quality: QualityPreset::Preserve,
            maximum_size_bytes: None,
            gif_fps: 15,
            gif_max_width: 800,
            system_volume: 100,
            microphone_volume: 100,
            mute_system: !has_system_audio,
            mute_microphone: !has_microphone_audio,
            mono: false,
        }
    }

    pub(super) fn clamp(&mut self, width: u32, height: u32, duration_ms: u64) {
        let duration = duration_ms.max(1);
        self.trim_start_ms = self.trim_start_ms.min(duration.saturating_sub(1));
        self.trim_end_ms = self.trim_end_ms.clamp(self.trim_start_ms + 1, duration);
        self.crop.width = self.crop.width.clamp(2, width.max(2));
        self.crop.height = self.crop.height.clamp(2, height.max(2));
        self.crop.x = self.crop.x.min(width.saturating_sub(self.crop.width));
        self.crop.y = self.crop.y.min(height.saturating_sub(self.crop.height));
        self.system_volume = self.system_volume.min(200);
        self.microphone_volume = self.microphone_volume.min(200);
        self.gif_fps = self.gif_fps.clamp(1, 30);
        self.gif_max_width = self.gif_max_width.max(320);
    }

    pub(super) fn output_dimensions(&self, source_width: u32, source_height: u32) -> (u32, u32) {
        let (width, height) = if self.crop_enabled {
            (self.crop.width, self.crop.height)
        } else {
            (source_width, source_height)
        };
        let max_height = match self.resolution {
            Resolution::Original => None,
            Resolution::P1080 => Some(1080),
            Resolution::P720 => Some(720),
            Resolution::Half => Some((height / 2).max(2)),
        };
        let dimensions = if let Some(max_height) = max_height
            && height > max_height
        {
            let scaled = ((width as u64 * max_height as u64) / height as u64) as u32;
            (scaled.max(2) & !1, max_height.max(2) & !1)
        } else {
            (width.max(2) & !1, height.max(2) & !1)
        };
        if self.format == ExportFormat::Gif && dimensions.0 > self.gif_max_width {
            let scaled_height =
                dimensions.1 as u64 * self.gif_max_width as u64 / dimensions.0 as u64;
            (
                self.gif_max_width.max(2) & !1,
                scaled_height.max(2) as u32 & !1,
            )
        } else {
            dimensions
        }
    }

    pub(super) fn specs(
        &self,
        source_width: u32,
        source_height: u32,
        has_system_audio: bool,
        has_microphone_audio: bool,
    ) -> (EditSpec, ExportSpec) {
        let (output_width, output_height) = self.output_dimensions(source_width, source_height);
        let edit = EditSpec {
            trim_start_ms: self.trim_start_ms,
            trim_end_ms: Some(self.trim_end_ms),
            crop: self.crop_enabled.then_some(self.crop),
            output_width: Some(output_width),
            output_height: Some(output_height),
            audio: AudioEdit {
                system_volume: f32::from(self.system_volume) / 100.0,
                microphone_volume: f32::from(self.microphone_volume) / 100.0,
                mute_system_audio: self.mute_system,
                mute_microphone: self.mute_microphone,
                mono_output: self.mono,
                source_has_system_audio: has_system_audio,
                source_has_microphone_audio: has_microphone_audio,
            },
        };
        let export = ExportSpec {
            format: self.format,
            quality: self.quality,
            max_size_bytes: self.maximum_size_bytes,
            frames_per_second: (self.format == ExportFormat::Gif).then_some(self.gif_fps),
            gif_max_colors: (self.format == ExportFormat::Gif).then_some(match self.quality {
                QualityPreset::Tiny => 64,
                QualityPreset::Small => 96,
                QualityPreset::Standard => 128,
                _ => 256,
            }),
        };
        (edit, export)
    }
}

pub(super) fn format_time(milliseconds: u64, tenths: bool) -> String {
    let seconds = milliseconds / 1_000;
    if tenths {
        format!(
            "{}:{:02}.{}",
            seconds / 60,
            seconds % 60,
            (milliseconds % 1_000) / 100
        )
    } else if seconds >= 3_600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3_600,
            (seconds % 3_600) / 60,
            seconds % 60
        )
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_clamps_both_sides_without_collapsing() {
        let mut settings = EditorSettings::new(1920, 1080, 10_000, true, false);
        settings.trim_start_ms = 12_000;
        settings.trim_end_ms = 3;
        settings.clamp(1920, 1080, 10_000);
        assert_eq!(
            (settings.trim_start_ms, settings.trim_end_ms),
            (9_999, 10_000)
        );
    }

    #[test]
    fn crop_and_resize_use_asymmetric_source_dimensions() {
        let mut settings = EditorSettings::new(1918, 1078, 1_000, false, false);
        settings.crop_enabled = true;
        settings.crop = CropRect {
            x: 117,
            y: 31,
            width: 1001,
            height: 701,
        };
        settings.resolution = Resolution::P720;
        assert_eq!(settings.output_dimensions(1918, 1078), (1000, 700));
        settings.resolution = Resolution::Half;
        // 1001 / 701 × floor(701 / 2) = 499.78; video dimensions are even.
        assert_eq!(settings.output_dimensions(1918, 1078), (498, 350));
    }

    #[test]
    fn gif_spec_never_carries_audio_or_silent_webm_support() {
        let mut settings = EditorSettings::new(800, 600, 1_000, true, false);
        settings.format = ExportFormat::Gif;
        settings.quality = QualityPreset::Tiny;
        let (_, export) = settings.specs(800, 600, true, false);
        assert_eq!(export.format, ExportFormat::Gif);
        assert_eq!(export.gif_max_colors, Some(64));
        assert_eq!(export.frames_per_second, Some(15));
    }

    #[test]
    fn independent_audio_tracks_keep_separate_editor_controls() {
        let settings = EditorSettings::new(800, 600, 1_000, true, true);
        let (edit, _) = settings.specs(800, 600, true, true);
        assert!(!settings.mute_system);
        assert!(!settings.mute_microphone);
        assert!(edit.audio.source_has_system_audio);
        assert!(edit.audio.source_has_microphone_audio);
    }

    #[test]
    fn phase_controls_reject_transitional_interleavings() {
        for phase in [
            HudPhase::Starting,
            HudPhase::Pausing,
            HudPhase::Finalizing,
            HudPhase::Discarding,
        ] {
            assert!(!phase.controllable());
        }
        assert!(HudPhase::Recording.controllable());
        assert!(HudPhase::Paused.controllable());
        assert!(HudPhase::Failed.controllable());
    }

    #[test]
    fn crop_corner_and_move_drags_clamp_asymmetrically() {
        let initial = CropRect {
            x: 100,
            y: 50,
            width: 400,
            height: 200,
        };
        assert_eq!(
            crop_after_drag(initial, CropHandle::Move, 900, -200, 1_000, 700, false),
            CropRect {
                x: 600,
                y: 0,
                ..initial
            }
        );
        assert_eq!(
            crop_after_drag(initial, CropHandle::SouthEast, 100, 100, 1_000, 700, true),
            CropRect {
                x: 100,
                y: 50,
                width: 600,
                height: 300,
            }
        );
        assert_eq!(
            crop_after_drag(initial, CropHandle::SouthEast, 900, 900, 1_000, 700, true),
            CropRect {
                x: 100,
                y: 50,
                width: 900,
                height: 450,
            }
        );
        assert_eq!(
            crop_after_drag(
                initial,
                CropHandle::NorthWest,
                -500,
                -500,
                1_000,
                700,
                false
            ),
            CropRect {
                x: 0,
                y: 0,
                width: 500,
                height: 250,
            }
        );
    }
}
