//! Platform recording engine dispatch shared by desktop hosts. Starting/stopping
//! a segment and enumerating devices may block; the host owns worker scheduling,
//! permissions, window exclusion and the recording session lifecycle.

#![forbid(unsafe_code)]

mod session;
pub use session::{FinalizedRecording, RecordingSession};
mod recovery;
pub use recovery::{RecordingRecovery, RecoveryDraft, RecoveryOutcome, RecoveryProgress};

use std::path::Path;

use captures_capture::DisplayDescriptor;
use captures_recording::{AudioDevice, RecordingOptions};
#[cfg(target_os = "macos")]
pub use captures_recording_macos::MacRecordingSegment as NativeRecordingSegment;
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub use captures_recording_xcap::XcapRecordingSegment as NativeRecordingSegment;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RecordingCapabilities {
    pub system_audio: bool,
    pub microphone: bool,
    pub cursor_control: bool,
    pub click_highlights: bool,
    pub controls_excluded: bool,
    /// False on Linux: the recording bar cannot be omitted from the capture stream.
    pub can_exclude_controls: bool,
}

/// Platforms that can keep the recording control bar out of the output.
pub const fn platform_can_exclude_recording_controls() -> bool {
    cfg!(any(target_os = "macos", target_os = "windows"))
}

/// Whether recording controls will be excluded from the next capture/recording
/// given the user's include preference.
pub const fn recording_controls_are_excluded(include_in_captures: bool) -> bool {
    platform_can_exclude_recording_controls() && !include_in_captures
}

impl RecordingCapabilities {
    pub fn current(include_recording_controls_in_captures: bool) -> Self {
        let can_exclude_controls = platform_can_exclude_recording_controls();
        let controls_excluded =
            recording_controls_are_excluded(include_recording_controls_in_captures);
        #[cfg(target_os = "macos")]
        {
            Self {
                system_audio: true,
                microphone: true,
                cursor_control: true,
                click_highlights: true,
                controls_excluded,
                can_exclude_controls,
            }
        }
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        {
            let pointer_features = captures_recording_xcap::pointer_features_available();
            Self {
                system_audio: true,
                // Capability describes platform support, not whether a device
                // happens to be connected during selector startup. The device
                // picker performs the one asynchronous enumeration it needs.
                microphone: true,
                cursor_control: pointer_features,
                click_highlights: pointer_features,
                controls_excluded,
                can_exclude_controls,
            }
        }
    }
}

pub fn microphone_devices() -> Vec<AudioDevice> {
    #[cfg(target_os = "macos")]
    {
        captures_recording_macos::microphone_devices()
    }
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    {
        captures_recording_xcap::microphone_devices()
    }
}

pub fn start_native_segment(
    options: &RecordingOptions,
    path: &Path,
    display: &DisplayDescriptor,
    exclude_captures_app: bool,
) -> Result<NativeRecordingSegment, String> {
    #[cfg(target_os = "macos")]
    {
        let _ = display;
        NativeRecordingSegment::start(options, path, exclude_captures_app)
            .map_err(|error| error.to_string())
    }
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    {
        let _ = exclude_captures_app;
        NativeRecordingSegment::start(options, path, display).map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_wire_fields_and_include_preference_are_preserved() {
        let can_exclude = cfg!(any(target_os = "macos", target_os = "windows"));
        #[cfg(target_os = "macos")]
        let pointer_features = true;
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        let pointer_features = captures_recording_xcap::pointer_features_available();
        for include in [false, true] {
            let capabilities = RecordingCapabilities::current(include);
            // Device presence must not change the platform capability contract.
            assert!(capabilities.microphone && capabilities.system_audio);
            assert_eq!(capabilities.can_exclude_controls, can_exclude);
            assert_eq!(capabilities.controls_excluded, can_exclude && !include);
            assert_eq!(
                serde_json::to_value(&capabilities).unwrap(),
                serde_json::json!({
                    "system_audio": true,
                    "microphone": true,
                    "cursor_control": pointer_features,
                    "click_highlights": pointer_features,
                    "controls_excluded": can_exclude && !include,
                    "can_exclude_controls": can_exclude,
                }),
            );
        }
    }
}
