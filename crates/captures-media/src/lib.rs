#![forbid(unsafe_code)]

mod export;
mod range;
mod toolchain;

pub use export::{
    AudioEdit, CropRect, EditSpec, ExportFormat, ExportProgress, ExportSpec, ExportStage,
    GifExportAttempt, MediaKind, MediaMetadata, QualityPreset, SizeBudget, SizeBudgetError,
    calculate_size_budget, estimate_sample_windows, extrapolate_sampled_size, gif_export_attempts,
};
pub use range::{ByteRange, ByteRangeError};
pub use toolchain::{
    CancelToken, ExportOutcome, MediaToolError, MediaToolchain, ProbeResult, RecordingAudioLayout,
    RecordingSegmentInput, TimelineSpriteSpec, export_preserves_source_bytes,
    visual_edit_is_identity,
};
