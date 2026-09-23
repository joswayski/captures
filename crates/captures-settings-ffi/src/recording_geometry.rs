//! Allocation-free recording crop and output-size geometry for native hosts.

use captures_media::{CropDragHandle, CropRect, CropResizeAxis};
use captures_recording::MaxResolution;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct RecordingDimensions {
    pub width: u32,
    pub height: u32,
}

fn valid_crop(crop: CropRect, source: RecordingDimensions) -> bool {
    source.width >= 2
        && source.height >= 2
        && crop.width >= 2
        && crop.height >= 2
        && crop.x <= source.width - 2
        && crop.y <= source.height - 2
}

/// Apply one source-pixel crop drag from the immutable original rectangle.
///
/// Handles 0..8 are move, N, NE, E, SE, S, SW, W and NW. Locked corners
/// retain both opposite edges; locked edge handles retain the opposite edge and
/// center the coupled dimension.
///
/// # Safety
/// `output` is null or writable aligned storage for one `CropRect`. False leaves
/// it untouched. Inputs and output are copied and no allocation is performed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_crop_after_drag_v1(
    initial: CropRect,
    source: RecordingDimensions,
    handle: u8,
    delta_x: f64,
    delta_y: f64,
    lock_aspect: bool,
    output: *mut CropRect,
) -> bool {
    if output.is_null() {
        return false;
    }
    let Ok(handle) = CropDragHandle::try_from(handle) else {
        return false;
    };
    let Some(crop) = initial.after_drag(
        source.width,
        source.height,
        handle,
        delta_x,
        delta_y,
        lock_aspect,
    ) else {
        return false;
    };
    // SAFETY: caller supplies writable output; all values are copied.
    unsafe { output.write(crop) };
    true
}

/// Resize one crop dimension while preserving its current aspect ratio.
///
/// Axis 0 changes width and axis 1 changes height. Staged crop dimensions may
/// exceed the source remainder; a valid call repairs them to fit from the origin.
///
/// # Safety
/// `output` is null or writable aligned storage for one `CropRect`. False leaves
/// it untouched. Inputs and output are copied and no allocation is performed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_crop_resize_locked_v1(
    crop: CropRect,
    source: RecordingDimensions,
    axis: u8,
    value: u32,
    output: *mut CropRect,
) -> bool {
    if output.is_null() || !valid_crop(crop, source) {
        return false;
    }
    let Ok(axis) = CropResizeAxis::try_from(axis) else {
        return false;
    };
    let resized = crop.resize_aspect_locked(source.width, source.height, axis, value);
    // SAFETY: caller supplies writable output; all values are copied.
    unsafe { output.write(resized) };
    true
}

/// Apply an existing recording max-resolution preset to pixel dimensions.
///
/// Presets 0, 1 and 2 are Original, 1080p and 720p respectively. Original
/// still applies the recording pipeline's even-dimension normalization.
///
/// # Safety
/// `output` is null or writable aligned storage for one `RecordingDimensions`.
/// False leaves it untouched. Inputs and output are copied and no allocation is
/// performed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_recording_max_resolution_constrain_v1(
    preset: u8,
    input: RecordingDimensions,
    output: *mut RecordingDimensions,
) -> bool {
    if output.is_null() || input.width == 0 || input.height == 0 {
        return false;
    }
    let preset = match preset {
        0 => MaxResolution::Original,
        1 => MaxResolution::P1080,
        2 => MaxResolution::P720,
        _ => return false,
    };
    let (width, height) = preset.constrain(input.width, input.height);
    // SAFETY: caller supplies writable output; all values are copied.
    unsafe { output.write(RecordingDimensions { width, height }) };
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    fn crop(x: u32, y: u32, width: u32, height: u32) -> CropRect {
        CropRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn crop_abi_preserves_orientation_rounding_and_repairs_bounds() {
        for (source, initial, axis, value, expected) in [
            (
                RecordingDimensions {
                    width: 320,
                    height: 180,
                },
                crop(10, 60, 160, 90),
                0,
                300,
                crop(10, 60, 213, 120),
            ),
            (
                RecordingDimensions {
                    width: 320,
                    height: 180,
                },
                crop(200, 6, 80, 60),
                1,
                170,
                crop(200, 6, 120, 90),
            ),
            (
                RecordingDimensions {
                    width: 400,
                    height: 300,
                },
                crop(20, 30, 101, 61),
                0,
                73,
                crop(20, 30, 73, 44),
            ),
            (
                RecordingDimensions {
                    width: 320,
                    height: 180,
                },
                crop(300, 20, 100, 50),
                0,
                100,
                crop(300, 20, 20, 10),
            ),
        ] {
            let mut output = crop(1, 2, 3, 4);
            // SAFETY: output is writable local storage.
            assert!(unsafe {
                captures_recording_crop_resize_locked_v1(initial, source, axis, value, &mut output)
            });
            assert_eq!(output, expected);
        }
    }

    #[test]
    fn crop_drag_abi_preserves_handle_orientation_locking_and_rounding() {
        let source = RecordingDimensions {
            width: 1_140,
            height: 692,
        };
        let initial = crop(100, 50, 400, 200);
        for (handle, delta_x, delta_y, locked, expected) in [
            (0, 900.0, 900.0, false, crop(740, 492, 400, 200)),
            (8, -150.0, 175.0, false, crop(0, 225, 500, 25)),
            (3, -900.0, 0.0, false, crop(100, 50, 2, 200)),
            (4, 120.0, 20.0, true, crop(100, 50, 520, 260)),
            (7, -250.0, 0.0, true, crop(0, 25, 500, 250)),
            (1, 0.0, -400.0, true, crop(50, 0, 500, 250)),
            (5, 0.0, 19.6, false, crop(100, 50, 400, 220)),
        ] {
            let mut output = crop(1, 2, 3, 4);
            // SAFETY: output is writable local storage.
            assert!(unsafe {
                captures_recording_crop_after_drag_v1(
                    initial,
                    source,
                    handle,
                    delta_x,
                    delta_y,
                    locked,
                    &mut output,
                )
            });
            assert_eq!(output, expected);
        }
    }

    #[test]
    fn resolution_abi_uses_existing_even_cap_and_no_upscale_rules() {
        for (preset, input, expected) in [
            (
                0,
                RecordingDimensions {
                    width: 753,
                    height: 597,
                },
                RecordingDimensions {
                    width: 752,
                    height: 596,
                },
            ),
            (
                1,
                RecordingDimensions {
                    width: 1_001,
                    height: 2_003,
                },
                RecordingDimensions {
                    width: 540,
                    height: 1_080,
                },
            ),
            (
                2,
                RecordingDimensions {
                    width: 1_283,
                    height: 719,
                },
                RecordingDimensions {
                    width: 1_282,
                    height: 718,
                },
            ),
            (
                0,
                RecordingDimensions {
                    width: 1,
                    height: 7,
                },
                RecordingDimensions {
                    width: 2,
                    height: 6,
                },
            ),
            (
                0,
                RecordingDimensions {
                    width: 7,
                    height: 1,
                },
                RecordingDimensions {
                    width: 6,
                    height: 2,
                },
            ),
        ] {
            let mut output = RecordingDimensions {
                width: 17,
                height: 19,
            };
            // SAFETY: output is writable local storage.
            assert!(unsafe {
                captures_recording_max_resolution_constrain_v1(preset, input, &mut output)
            });
            assert_eq!(output, expected);
        }
    }

    #[test]
    fn invalid_abi_inputs_leave_output_untouched() {
        let source = RecordingDimensions {
            width: 320,
            height: 180,
        };
        let sentinel_crop = crop(1, 2, 3, 4);
        let mut output_crop = sentinel_crop;
        for (initial, axis) in [
            (crop(0, 0, 160, 90), 2),
            (crop(319, 0, 2, 2), 0),
            (crop(0, 179, 2, 2), 1),
            (crop(0, 0, 1, 2), 0),
        ] {
            // SAFETY: output is writable local storage.
            assert!(!unsafe {
                captures_recording_crop_resize_locked_v1(
                    initial,
                    source,
                    axis,
                    40,
                    &mut output_crop,
                )
            });
            assert_eq!(output_crop, sentinel_crop);
        }
        for (initial, handle, delta_x, delta_y) in [
            (crop(0, 0, 160, 90), 9, 1.0, 1.0),
            (crop(319, 0, 2, 2), 0, 1.0, 1.0),
            (crop(0, 0, 1, 2), 3, 1.0, 0.0),
            (crop(0, 0, 160, 90), 3, f64::NAN, 0.0),
            (crop(0, 0, 160, 90), 3, 0.0, f64::INFINITY),
        ] {
            // SAFETY: output is writable local storage.
            assert!(!unsafe {
                captures_recording_crop_after_drag_v1(
                    initial,
                    source,
                    handle,
                    delta_x,
                    delta_y,
                    false,
                    &mut output_crop,
                )
            });
            assert_eq!(output_crop, sentinel_crop);
        }

        let sentinel_dimensions = RecordingDimensions {
            width: 17,
            height: 19,
        };
        let mut output_dimensions = sentinel_dimensions;
        // SAFETY: output is writable local storage; null output is a supported failure.
        unsafe {
            assert!(!captures_recording_max_resolution_constrain_v1(
                3,
                source,
                &mut output_dimensions,
            ));
            assert!(!captures_recording_max_resolution_constrain_v1(
                0,
                RecordingDimensions {
                    width: 0,
                    height: 180,
                },
                &mut output_dimensions,
            ));
            assert!(!captures_recording_max_resolution_constrain_v1(
                0,
                RecordingDimensions {
                    width: 320,
                    height: 0,
                },
                &mut output_dimensions,
            ));
            assert!(!captures_recording_crop_resize_locked_v1(
                crop(0, 0, 160, 90),
                source,
                0,
                40,
                ptr::null_mut(),
            ));
            assert!(!captures_recording_crop_after_drag_v1(
                crop(0, 0, 160, 90),
                source,
                0,
                1.0,
                1.0,
                false,
                ptr::null_mut(),
            ));
            assert!(!captures_recording_max_resolution_constrain_v1(
                0,
                source,
                ptr::null_mut(),
            ));
        }
        assert_eq!(output_dimensions, sentinel_dimensions);
    }
}
