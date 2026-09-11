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
