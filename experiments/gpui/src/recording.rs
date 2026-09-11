//! GPUI recording lifecycle, fixed-glass HUD, recovery, and media editor.
//!
//! Capture and media work runs on GPUI's background executor. Active segments
//! are finalized before pause, screenshot, mute, restart, stop, or discard, so
//! every manifest only marks media complete after its MP4 container is safe.

mod editor;
mod lifecycle;
mod model;
mod recovery;

use anyhow::Result;
use captures_capture::DisplayDescriptor;
use captures_recording::RecordingOptions;
use gpui::App;
use std::path::PathBuf;

#[cfg(target_os = "macos")]
use captures_recording_macos::MacRecordingSegment as RecordingSegment;
#[cfg(any(target_os = "linux", target_os = "windows"))]
use captures_recording_xcap::XcapRecordingSegment as RecordingSegment;

pub(crate) fn microphone_devices() -> Vec<captures_recording::AudioDevice> {
    #[cfg(target_os = "macos")]
    return captures_recording_macos::microphone_devices();
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    return captures_recording_xcap::microphone_devices();
}

fn start_segment(
    options: &RecordingOptions,
    path: &std::path::Path,
    display: &DisplayDescriptor,
    exclude_app: bool,
) -> Result<RecordingSegment, String> {
    #[cfg(target_os = "macos")]
    {
        let _ = display;
        if !options.audio.microphone_muted
            && options.audio.microphone_device_id.is_some()
            && !captures_recording_macos::request_microphone_access()
        {
            return Err("Microphone access was denied. Allow Captures GPUI in System Settings → Privacy & Security → Microphone, or record without a microphone.".into());
        }
        // Permission prompts can stay open while the desktop locks.
        if !captures_session::capture_session_available() {
            return Err("Recording cancelled: desktop session became locked or inactive.".into());
        }
        captures_capture::XcapBackend
            .ensure_permission(false)
            .map_err(|error| error.to_string())?;
        RecordingSegment::start(options, path, exclude_app).map_err(|error| error.to_string())
    }
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        let _ = exclude_app;
        RecordingSegment::start(options, path, display).map_err(|error| error.to_string())
    }
}

/// Starts countdown and recording, then presents the always-dark recording HUD.
pub fn start(
    options: RecordingOptions,
    display: DisplayDescriptor,
    output_directory: PathBuf,
    cx: &mut App,
) -> Result<()> {
    lifecycle::start(options, display, output_directory, cx)
}

/// Opens a complete video/GIF editor for an existing recording.
pub fn open(path: PathBuf, cx: &mut App) -> Result<()> {
    editor::open(path, cx)
}

/// Presents recoverable interrupted sessions, if any exist.
pub fn recover(cx: &mut App) -> Result<()> {
    recovery::open(cx)
}

/// Restores a HUD compacted with Hide controls and activates its window.
pub fn restore_controls(cx: &mut App) {
    lifecycle::restore_controls(cx);
}

/// Requests application quit without abandoning an active recording.
///
/// An active session prompts to safely save or explicitly discard first. If
/// finalization is already running, quitting is deferred until it succeeds.
pub fn request_quit(cx: &mut App) {
    lifecycle::request_quit(cx);
}
