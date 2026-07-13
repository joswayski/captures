#![forbid(unsafe_code)]

mod backend;
mod error;
mod geometry;
mod model;

pub use backend::XcapBackend;
pub use error::{CaptureError, CaptureResult};
pub use geometry::{LogicalRect, PhysicalRect};
pub use model::{CaptureMode, DisplayDescriptor, WindowDescriptor};
