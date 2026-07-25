use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingKind {
    Video,
    Gif,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingState {
    Selecting,
    Countdown,
    Recording,
    Paused,
    Finalizing,
    Ready,
    Editor,
    Failed,
    Discarded,
}

impl RecordingState {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Ready | Self::Failed | Self::Discarded)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CaptureRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl CaptureRect {
    pub const fn is_valid(self) -> bool {
        self.width > 0 && self.height > 0
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RecordingTarget {
    Display {
        display_id: String,
    },
    Region {
        display_id: String,
        rect: CaptureRect,
    },
    Window {
        window_id: String,
    },
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MaxResolution {
    #[default]
    Original,
    P1080,
    P720,
}

impl MaxResolution {
    pub const fn height(self) -> Option<u32> {
        match self {
            Self::Original => None,
            Self::P1080 => Some(1_080),
            Self::P720 => Some(720),
        }
    }

    pub fn constrain(self, width: u32, height: u32) -> (u32, u32) {
        let Some(max_height) = self.height() else {
            return even_dimensions(width, height);
        };
        if height <= max_height {
            return even_dimensions(width, height);
        }

        let scale = f64::from(max_height) / f64::from(height);
        let scaled_width = (f64::from(width) * scale).round().max(2.0) as u32;
        even_dimensions(scaled_width, max_height)
    }
}

fn even_dimensions(width: u32, height: u32) -> (u32, u32) {
    (width.max(2) & !1, height.max(2) & !1)
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AudioOptions {
    #[serde(default)]
    pub capture_system_audio: bool,
    #[serde(default)]
    pub microphone_device_id: Option<String>,
    #[serde(default)]
    pub mono_output: bool,
    #[serde(default = "default_audio_volume_percent")]
    pub system_volume_percent: u16,
    #[serde(default = "default_audio_volume_percent")]
    pub microphone_volume_percent: u16,
    #[serde(default)]
    pub microphone_muted: bool,
}

impl Default for AudioOptions {
    fn default() -> Self {
        Self {
            capture_system_audio: false,
            microphone_device_id: None,
            mono_output: false,
            system_volume_percent: 100,
            microphone_volume_percent: 100,
            microphone_muted: false,
        }
    }
}

const fn default_audio_volume_percent() -> u16 {
    100
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GifOptions {
    pub max_width: u32,
    pub max_colors: u16,
    pub optimize: bool,
}

impl Default for GifOptions {
    fn default() -> Self {
        Self {
            max_width: 800,
            max_colors: 256,
            optimize: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RecordingOptions {
    pub kind: RecordingKind,
    pub target: RecordingTarget,
    pub frames_per_second: u16,
    pub max_resolution: MaxResolution,
    pub countdown_seconds: u8,
    pub show_cursor: bool,
    #[serde(default)]
    pub highlight_clicks: bool,
    #[serde(default)]
    pub show_keystrokes: bool,
    #[serde(default)]
    pub audio: AudioOptions,
    #[serde(default)]
    pub gif: GifOptions,
}

impl RecordingOptions {
    pub fn validate(&self) -> Result<(), &'static str> {
        let valid_fps = match self.kind {
            RecordingKind::Video => matches!(self.frames_per_second, 15 | 30 | 60),
            RecordingKind::Gif => (1..=30).contains(&self.frames_per_second),
        };
        if !valid_fps {
            return Err(match self.kind {
                RecordingKind::Video => "video FPS must be 15, 30, or 60",
                RecordingKind::Gif => "GIF FPS must be between 1 and 30",
            });
        }
        if self.countdown_seconds > 10 {
            return Err("recording countdown must be at most 10 seconds");
        }
        if matches!(self.target, RecordingTarget::Region { rect, .. } if !rect.is_valid()) {
            return Err("recording region must be larger than zero pixels");
        }
        if self.kind == RecordingKind::Gif
            && (self.audio.capture_system_audio || self.audio.microphone_device_id.is_some())
        {
            return Err("GIF recordings cannot capture audio");
        }
        if self.audio.system_volume_percent > 200 || self.audio.microphone_volume_percent > 200 {
            return Err("recording audio volume must be between 0 and 200 percent");
        }
        if self.gif.max_width < 320 || self.gif.max_colors < 64 || self.gif.max_colors > 256 {
            return Err("GIF settings are outside their supported range");
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioDeviceKind {
    Default,
    Microphone,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub kind: AudioDeviceKind,
    pub is_default: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RecordingSessionSnapshot {
    pub id: String,
    pub state: RecordingState,
    pub options: RecordingOptions,
    pub elapsed_ms: u64,
    pub countdown_remaining_seconds: Option<u8>,
    pub warning: Option<String>,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{
        AudioOptions, CaptureRect, GifOptions, MaxResolution, RecordingKind, RecordingOptions,
        RecordingTarget,
    };

    fn options(kind: RecordingKind) -> RecordingOptions {
        RecordingOptions {
            kind,
            target: RecordingTarget::Region {
                display_id: "display-1".to_owned(),
                rect: CaptureRect {
                    x: 10,
                    y: 20,
                    width: 1_919,
                    height: 1_080,
                },
            },
            frames_per_second: 30,
            max_resolution: MaxResolution::P1080,
            countdown_seconds: 3,
            show_cursor: true,
            highlight_clicks: false,
            show_keystrokes: false,
            audio: AudioOptions::default(),
            gif: GifOptions::default(),
        }
    }

    #[test]
    fn constrains_resolution_without_upscaling_and_uses_even_dimensions() {
        assert_eq!(MaxResolution::P1080.constrain(3_840, 2_160), (1_920, 1_080));
        assert_eq!(MaxResolution::P1080.constrain(1_919, 1_079), (1_918, 1_078));
        assert_eq!(MaxResolution::Original.constrain(753, 597), (752, 596));
    }

    #[test]
    fn rejects_invalid_fps_and_gif_audio() {
        let mut recording = options(RecordingKind::Video);
        recording.frames_per_second = 24;
        assert_eq!(recording.validate(), Err("video FPS must be 15, 30, or 60"));

        let mut gif = options(RecordingKind::Gif);
        gif.frames_per_second = 24;
        assert!(gif.validate().is_ok());

        gif.audio.capture_system_audio = true;
        assert_eq!(gif.validate(), Err("GIF recordings cannot capture audio"));
    }
}
