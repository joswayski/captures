use thiserror::Error;

pub type CaptureResult<T> = Result<T, CaptureError>;

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("screen capture permission was requested")]
    PermissionRequestStarted,
    #[error("screen capture permission was denied")]
    PermissionDenied,
    #[error("screen capture is unavailable while the desktop session is locked or inactive")]
    SessionUnavailable,
    #[error("the requested capture target is not available")]
    TargetUnavailable,
    #[error("the requested capture mode is not supported")]
    Unsupported,
    #[error(
        "Could not isolate that window while another window was covering it. Bring the window forward and try again."
    )]
    WindowOccluded,
    #[error("Could not capture that window (empty frame). Try Region capture.")]
    WindowEmpty,
    #[error("capture failed: {0}")]
    Backend(String),
    #[error("image operation failed: {0}")]
    Image(String),
}
