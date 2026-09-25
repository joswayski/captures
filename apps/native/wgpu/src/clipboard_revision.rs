//! Clipboard revisions for mini-preview "Copied to clipboard" ownership.
//!
//! Windows exposes a system sequence number that changes on every write by any
//! app. Elsewhere this host counts its own writes; external replacements are
//! detected by periodically comparing clipboard pixels (see `NEEDS_PIXEL_CHECK`).
use std::sync::atomic::{AtomicI64, Ordering};

static APPLICATION_REVISION: AtomicI64 = AtomicI64::new(0);

/// Whether external clipboard changes must be found by comparing pixels.
pub const NEEDS_PIXEL_CHECK: bool = cfg!(not(target_os = "windows"));

#[cfg(target_os = "windows")]
fn system_revision() -> Option<i64> {
    clipboard_win::seq_num().map(|revision| i64::from(revision.get()))
}

#[cfg(not(target_os = "windows"))]
fn system_revision() -> Option<i64> {
    None
}

pub fn current() -> i64 {
    system_revision().unwrap_or_else(|| APPLICATION_REVISION.load(Ordering::Acquire))
}

/// Record a successful clipboard write by this host and return its revision.
pub fn after_write() -> i64 {
    if let Some(revision) = system_revision() {
        APPLICATION_REVISION.store(revision, Ordering::Release);
        return revision;
    }
    APPLICATION_REVISION
        .fetch_add(1, Ordering::AcqRel)
        .wrapping_add(1)
}

/// Advance past `revision` after a pixel check found another app's contents,
/// unless a newer write by this host already did.
pub fn invalidate(revision: i64) -> bool {
    APPLICATION_REVISION
        .compare_exchange(
            revision,
            revision.wrapping_add(1),
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_ok()
}
