#![deny(unsafe_op_in_unsafe_fn)]

use std::path::{Path, PathBuf};

use captures_recording::{AudioDevice, RecordingOptions, RecordingSegmentInfo};
use thiserror::Error;

#[cfg(target_os = "macos")]
mod microphone;
#[cfg(target_os = "macos")]
mod native;
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod writer;

#[derive(Debug, Error)]
pub enum MacRecordingError {
    #[error("ScreenCaptureKit recording is unavailable on this platform")]
    RecordingOutputUnavailable,
    #[error("recording target is no longer available")]
    TargetUnavailable,
    #[error("recording path is not valid Unicode")]
    InvalidOutputPath,
    #[error("recording configuration is invalid: {0}")]
    InvalidOptions(String),
    #[error("ScreenCaptureKit failed: {0}")]
    ScreenCaptureKit(String),
    #[error("{0}")]
    RecordingFailed(String),
    #[error("microphone capture failed: {0}")]
    Microphone(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type MacRecordingResult<T> = Result<T, MacRecordingError>;

/// A single independently playable recording segment.
///
/// Captures closes the current segment on pause, restart, and display changes.
/// Keeping that boundary here makes an interrupted session recoverable without
/// relying on an in-memory encoder state.
pub struct MacRecordingSegment {
    #[cfg(target_os = "macos")]
    inner: native::NativeRecordingSegment,
    #[cfg(target_os = "macos")]
    microphone: Option<microphone::MicrophoneSegment>,
    #[cfg(not(target_os = "macos"))]
    _private: (),
}

impl MacRecordingSegment {
    pub fn start(
        options: &RecordingOptions,
        output_path: &Path,
        exclude_captures_app: bool,
    ) -> MacRecordingResult<Self> {
        options
            .validate()
            .map_err(|error| MacRecordingError::InvalidOptions(error.to_owned()))?;
        #[cfg(target_os = "macos")]
        {
            let mut screen_options = options.clone();
            screen_options.audio.microphone_device_id = None;
            screen_options.audio.microphone_muted = true;
            let inner = native::NativeRecordingSegment::start(
                &screen_options,
                output_path,
                exclude_captures_app,
            )?;
            let microphone = if options.audio.microphone_muted {
                None
            } else if let Some(device_id) = options.audio.microphone_device_id.as_deref() {
                let microphone_path = microphone_path(output_path);
                let offset_ms = inner.elapsed_since_start_ms();
                match microphone::MicrophoneSegment::start(device_id, &microphone_path, offset_ms) {
                    Ok(microphone) => Some(microphone),
                    Err(error) => {
                        let _ = inner.discard();
                        return Err(error);
                    }
                }
            } else {
                None
            };
            Ok(Self { inner, microphone })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (output_path, exclude_captures_app);
            Err(MacRecordingError::RecordingOutputUnavailable)
        }
    }

    pub fn dimensions(&self) -> (u32, u32) {
        #[cfg(target_os = "macos")]
        {
            self.inner.dimensions()
        }
        #[cfg(not(target_os = "macos"))]
        {
            (0, 0)
        }
    }

    pub fn stop(self) -> MacRecordingResult<RecordingSegmentInfo> {
        #[cfg(target_os = "macos")]
        {
            let screen = self.inner.stop();
            let microphone = self.microphone.map(microphone::MicrophoneSegment::stop);
            let mut info = screen?;
            if let Some(microphone) = microphone {
                let microphone = microphone?;
                info.microphone_path = microphone.path;
                info.microphone_offset_ms = microphone.offset_ms;
                info.microphone_warning = microphone.warning;
            }
            Ok(info)
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(MacRecordingError::RecordingOutputUnavailable)
        }
    }

    pub fn discard(self) -> MacRecordingResult<()> {
        #[cfg(target_os = "macos")]
        {
            let screen_result = self.inner.discard();
            if let Some(microphone) = self.microphone {
                let _ = microphone.discard();
            }
            screen_result
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(())
        }
    }

    pub fn warning(&self) -> Option<String> {
        #[cfg(target_os = "macos")]
        {
            self.inner.warning().or_else(|| {
                self.microphone
                    .as_ref()
                    .and_then(microphone::MicrophoneSegment::warning)
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    }

    pub fn microphone_level(&self) -> f32 {
        #[cfg(target_os = "macos")]
        {
            self.microphone
                .as_ref()
                .map_or(0.0, microphone::MicrophoneSegment::level)
        }
        #[cfg(not(target_os = "macos"))]
        {
            0.0
        }
    }

    pub fn microphone_draft_info(&self) -> Option<(PathBuf, i64)> {
        #[cfg(target_os = "macos")]
        {
            self.microphone
                .as_ref()
                .map(microphone::MicrophoneSegment::draft_info)
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    }

    pub const fn system_audio_draft_info(&self) -> Option<(PathBuf, i64)> {
        None
    }
}

pub fn microphone_devices() -> Vec<AudioDevice> {
    #[cfg(target_os = "macos")]
    {
        native::microphone_devices()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

#[cfg(target_os = "macos")]
pub fn microphone_authorized() -> bool {
    // SAFETY: the Swift bridge only queries AVFoundation's current TCC status.
    unsafe { captures_microphone_authorized() }
}

#[cfg(target_os = "macos")]
pub fn microphone_can_request() -> bool {
    // SAFETY: the Swift bridge only queries AVFoundation's current TCC status.
    unsafe { captures_microphone_can_request() }
}

#[cfg(target_os = "macos")]
pub fn request_microphone_access() -> bool {
    // SAFETY: the Swift bridge shows the system prompt if needed, then returns
    // whether this process is authorized.
    unsafe { captures_microphone_request() }
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn captures_microphone_authorized() -> bool;
    fn captures_microphone_can_request() -> bool;
    fn captures_microphone_request() -> bool;
}

pub fn play_start_chime() {
    #[cfg(target_os = "macos")]
    // AudioServices plays from this process, so ScreenCaptureKit's
    // excludesCurrentProcessAudio setting also excludes the cue.
    unsafe {
        audio_services_play_system_sound(1113);
    }
}

#[cfg(target_os = "macos")]
#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    #[link_name = "AudioServicesPlaySystemSound"]
    fn audio_services_play_system_sound(sound_id: u32);
}

#[cfg(target_os = "macos")]
fn microphone_path(output_path: &Path) -> PathBuf {
    let stem = output_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("segment");
    output_path.with_file_name(format!("{stem}.mic.wav"))
}

#[cfg(test)]
mod tests {
    use super::MacRecordingError;

    #[test]
    fn recording_failures_only_show_the_actionable_message() {
        assert_eq!(
            MacRecordingError::RecordingFailed(
                "the recording did not contain a complete video frame".to_owned()
            )
            .to_string(),
            "the recording did not contain a complete video frame"
        );
    }
}
