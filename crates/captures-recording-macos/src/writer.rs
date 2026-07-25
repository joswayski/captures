use std::{ffi::CString, os::raw::c_char, path::Path, sync::Arc};

use screencapturekit::cm::CMSampleBuffer;

use crate::{MacRecordingError, MacRecordingResult};

unsafe extern "C" {
    fn captures_media_writer_create(
        path: *const c_char,
        width: u32,
        height: u32,
        frames_per_second: u32,
        captures_audio: bool,
        mono: bool,
    ) -> *mut std::ffi::c_void;
    fn captures_media_writer_append(
        handle: *mut std::ffi::c_void,
        sample: *mut std::ffi::c_void,
        kind: i32,
    ) -> bool;
    fn captures_media_writer_finish(handle: *mut std::ffi::c_void) -> bool;
    fn captures_media_writer_duration_ms(handle: *mut std::ffi::c_void) -> u64;
    fn captures_media_writer_dropped_frames(handle: *mut std::ffi::c_void) -> u64;
    fn captures_media_writer_error(
        handle: *mut std::ffi::c_void,
        buffer: *mut c_char,
        capacity: isize,
    ) -> isize;
    fn captures_media_writer_release(handle: *mut std::ffi::c_void);
}

#[derive(Clone)]
pub struct MediaWriter(Arc<MediaWriterInner>);

struct MediaWriterInner {
    handle: usize,
}

impl MediaWriter {
    pub fn new(
        path: &Path,
        width: u32,
        height: u32,
        frames_per_second: u32,
        captures_audio: bool,
        mono: bool,
    ) -> MacRecordingResult<Self> {
        let path = path.to_str().ok_or(MacRecordingError::InvalidOutputPath)?;
        let path = CString::new(path).map_err(|_| MacRecordingError::InvalidOutputPath)?;
        // SAFETY: the path is a live NUL-terminated C string for the duration
        // of the call. The Swift bridge either returns a retained writer or
        // null; the retained writer is released exactly once by Drop.
        let handle = unsafe {
            captures_media_writer_create(
                path.as_ptr(),
                width,
                height,
                frames_per_second,
                captures_audio,
                mono,
            )
        };
        if handle.is_null() {
            return Err(MacRecordingError::RecordingFailed(
                "Apple's H.264 writer could not be created".to_owned(),
            ));
        }
        Ok(Self(Arc::new(MediaWriterInner {
            handle: handle as usize,
        })))
    }

    pub fn append_video(&self, sample: &CMSampleBuffer) -> bool {
        self.append(sample, 0)
    }

    pub fn append_audio(&self, sample: &CMSampleBuffer) -> bool {
        self.append(sample, 1)
    }

    fn append(&self, sample: &CMSampleBuffer, kind: i32) -> bool {
        // SAFETY: the handle owns a live Swift writer and ScreenCaptureKit
        // keeps the CMSampleBuffer alive for this synchronous call.
        unsafe { captures_media_writer_append(self.handle(), sample.as_ptr(), kind) }
    }

    pub fn finish(&self) -> MacRecordingResult<()> {
        // SAFETY: the handle remains retained by self for the whole call.
        if unsafe { captures_media_writer_finish(self.handle()) } {
            Ok(())
        } else {
            Err(MacRecordingError::RecordingFailed(self.error_message()))
        }
    }

    pub fn duration_ms(&self) -> u64 {
        // SAFETY: the handle remains retained by self for the whole call.
        unsafe { captures_media_writer_duration_ms(self.handle()) }
    }

    pub fn dropped_frames(&self) -> u64 {
        // SAFETY: the handle remains retained by self for the whole call.
        unsafe { captures_media_writer_dropped_frames(self.handle()) }
    }

    pub fn error_message(&self) -> String {
        let mut buffer = [0_u8; 1_024];
        // SAFETY: buffer is writable for its full capacity and the handle is
        // retained. The bridge always NUL-terminates when capacity is nonzero.
        let length = unsafe {
            captures_media_writer_error(
                self.handle(),
                buffer.as_mut_ptr().cast(),
                isize::try_from(buffer.len()).unwrap_or(isize::MAX),
            )
        };
        let length = usize::try_from(length)
            .unwrap_or_default()
            .min(buffer.len());
        let message = String::from_utf8_lossy(&buffer[..length]).into_owned();
        if message.is_empty() {
            "Apple's media writer failed without an error message".to_owned()
        } else {
            message
        }
    }

    fn handle(&self) -> *mut std::ffi::c_void {
        self.0.handle as *mut std::ffi::c_void
    }
}

impl Drop for MediaWriterInner {
    fn drop(&mut self) {
        // SAFETY: this is the final Arc owner and create returned this exact
        // retained pointer, so the bridge release balances it once.
        unsafe { captures_media_writer_release(self.handle as *mut std::ffi::c_void) };
    }
}
