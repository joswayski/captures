#![forbid(unsafe_code)]

mod draft;
mod model;
mod session;

pub use draft::{DraftStore, RecordingDraftManifest, RecordingSegmentManifest, RecoveryError};
pub use model::{
    AudioDevice, AudioDeviceKind, AudioOptions, CaptureRect, GifOptions, MaxResolution,
    RecordingKind, RecordingOptions, RecordingSegmentInfo, RecordingSessionSnapshot,
    RecordingState, RecordingTarget,
};
pub use session::{RecordingCoordinator, RecordingError, RecordingResult};
