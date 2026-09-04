#[cfg(any(target_os = "windows", target_os = "linux"))]
mod audio;
#[cfg(any(target_os = "windows", target_os = "linux", test))]
mod overlay;
#[cfg(any(target_os = "windows", target_os = "linux"))]
mod pointer;
#[cfg(target_os = "linux")]
mod system_audio_linux;
#[cfg(any(target_os = "windows", target_os = "linux", test))]
mod transform;

#[cfg(any(target_os = "windows", target_os = "linux"))]
mod segment;

#[cfg(any(target_os = "windows", target_os = "linux"))]
pub use audio::microphone_devices;
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub use pointer::pointer_features_available;
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub use segment::{XcapRecordingError, XcapRecordingResult, XcapRecordingSegment};
