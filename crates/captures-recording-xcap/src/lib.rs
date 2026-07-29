#![forbid(unsafe_code)]

#[cfg(any(target_os = "windows", target_os = "linux", test))]
mod transform;

#[cfg(any(target_os = "windows", target_os = "linux"))]
mod segment;

#[cfg(any(target_os = "windows", target_os = "linux"))]
pub use segment::{XcapRecordingError, XcapRecordingResult, XcapRecordingSegment};
